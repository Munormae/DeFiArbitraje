use anyhow::{Result, anyhow};
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::{Address, Bytes, U256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::approvals::ensure_approvals;
use crate::calldata::encode_route_calldata;
use crate::config::{Config, Network};
use crate::exec::{Executor, TxOpts};
use crate::metrics::{METRIC_PROFITABLE_FOUND, METRIC_ROUTES_SCANNED, METRIC_TX_SENT};
use crate::network::{ChainClient, MultiChain};
use crate::utils::{bps, parse_addr, u256_from_decimals};

// ===== Route Planner =====
#[derive(Clone)]
pub struct RoutePlanner {
    pub cfg: Config,
}
impl RoutePlanner {
    pub fn from_config(cfg: &Config) -> Self {
        Self { cfg: cfg.clone() }
    }
}

// ===== Strategy Engine =====
pub struct StrategyEngine {
    cfg: Config,
    chains: Arc<MultiChain>,
    planner: Arc<RoutePlanner>,
    pnl: PnLTracker,
    // Исполнители по сетям (SignerMiddleware)
    executors: HashMap<u64, Arc<Executor<Provider<Http>, LocalWallet>>>,
}

impl StrategyEngine {
    pub async fn new(
        cfg: Config,
        chains: Arc<MultiChain>,
        planner: Arc<RoutePlanner>,
    ) -> Result<Self> {
        let mut executors: HashMap<u64, Arc<Executor<Provider<Http>, LocalWallet>>> =
            HashMap::new();

        for (chain_id, client) in chains.clients.iter() {
            let env_key_exec = format!("EXECUTOR_{}", chain_id);
            if std::env::var(&env_key_exec).is_err() {
                tracing::debug!(
                    "Executor не задан для chain_id={} (нет ENV {})",
                    chain_id,
                    env_key_exec
                );
                continue;
            }

            match signer_middleware_for_chain(client.provider.clone(), *chain_id) {
                Ok(signer_client) => {
                    let exec = Executor::new(signer_client.clone()).await?;
                    executors.insert(*chain_id, Arc::new(exec));
                    tracing::info!("Executor инициализирован для chain_id={}", chain_id);

                    if cfg.global.execution.approve_spend_on_start {
                        let mut spenders: HashSet<Address> = HashSet::new();
                        for d in &client.cfg.dexes {
                            if let Some(r) = &d.router {
                                if let Ok(a) = parse_addr(r) {
                                    spenders.insert(a);
                                }
                            }
                            if let Some(r) = &d.swap_router02 {
                                if let Ok(a) = parse_addr(r) {
                                    spenders.insert(a);
                                }
                            }
                            if let Some(r) = &d.universal_router {
                                if let Ok(a) = parse_addr(r) {
                                    spenders.insert(a);
                                }
                            }
                            if let Some(r) = &d.smart_router {
                                if let Ok(a) = parse_addr(r) {
                                    spenders.insert(a);
                                }
                            }
                        }
                        let spenders: Vec<Address> = spenders.into_iter().collect();
                        let tokens: Vec<Address> = client
                            .cfg
                            .tokens
                            .values()
                            .filter_map(|t| parse_addr(&t.address).ok())
                            .collect();
                        let min_allowance = U256::from_dec_str("1000000000000000000000000")?;
                        ensure_approvals(
                            signer_client.clone(),
                            &client.cfg,
                            tokens,
                            spenders,
                            min_allowance,
                        )
                        .await?;
                    }
                }
                Err(e) => tracing::warn!("Signer init failed for chain_id={}: {e:#}", chain_id),
            }
        }

        Ok(Self {
            cfg,
            chains,
            planner,
            pnl: PnLTracker::new(),
            executors,
        })
    }

    pub async fn scan_and_execute(&mut self) -> Result<()> {
        let chain_ids: Vec<u64> = self.cfg.networks.iter().map(|n| n.chain_id).collect();

        for chain_id in chain_ids {
            if let Some(client) = self.chains.clients.get(&chain_id).cloned() {
                self.scan_network(&client).await?;
            }
        }
        Ok(())
    }

    /// per-network override slippage_bps
    fn network_slippage_bps(&self, chain_id: u64) -> u32 {
        let default_slip = self.planner.cfg.global.quote.slippage_bps_default;
        self.planner
            .cfg
            .networks
            .iter()
            .find(|n| n.chain_id == chain_id)
            .and_then(|n| n.strategy_overrides.as_ref())
            .and_then(|ov| ov.slippage_bps)
            .unwrap_or(default_slip)
    }

    /// per-network override min_profit_bps
    fn network_min_profit_bps(&self, chain_id: u64) -> u32 {
        self.planner
            .cfg
            .networks
            .iter()
            .find(|n| n.chain_id == chain_id)
            .and_then(|n| n.strategy_overrides.as_ref())
            .and_then(|ov| ov.min_profit_bps)
            .unwrap_or(0)
    }

    async fn scan_network(&mut self, client: &ChainClient) -> Result<()> {
        let cooldown_sec = self.cfg.safety.circuit_breaker.cooldown_sec;
        if self.pnl.should_cooldown(cooldown_sec) {
            let remaining = self
                .pnl
                .last_loss_ts
                .map(|ts| cooldown_sec.saturating_sub(ts.elapsed().as_secs()))
                .unwrap_or(cooldown_sec);
            tracing::warn!(
                chain = client.cfg.chain_id,
                consec_losses = self.pnl.consec_losses,
                remaining,
                "cooldown active ({}s). Skip",
                remaining
            );
            return Ok(());
        }

        // ----- circuit breaker по сериям неудач -----
        let max_losses = self.cfg.safety.circuit_breaker.max_losses_in_row;
        if self.pnl.consec_losses >= max_losses {
            tracing::warn!(
                chain = client.cfg.chain_id,
                consec_losses = self.pnl.consec_losses,
                "circuit breaker: skipping network (max_losses_in_row={})",
                max_losses
            );
            return Ok(());
        }

        let slip_bps = self.network_slippage_bps(client.cfg.chain_id);
        let min_profit_bps = self.network_min_profit_bps(client.cfg.chain_id);
        let slip_frac = bps(slip_bps as f64);
        let min_profit_frac = bps(min_profit_bps as f64);

        tracing::debug!(
            chain = client.cfg.chain_id,
            slip_bps,
            min_profit_bps,
            slip_frac,
            min_profit_frac,
            "network overrides"
        );

        // -------- Кросс-DEX пары
        if let Some(routes) = &client.cfg.routes_cross_dex {
            for r in routes {
                if self.skip_pair_by_risk(&client.cfg, &r.pair[0], &r.pair[1]) {
                    continue;
                }

                METRIC_ROUTES_SCANNED.inc();
                let a = addr_of(&client.cfg, &r.pair[0])?;
                let b = addr_of(&client.cfg, &r.pair[1])?;

                // Эвристика ликвидности (пока без резервов — пропускаем проверку)
                let _ok_liq = self.meets_min_liquidity_hint(
                    &client.cfg,
                    &r.pair[0],
                    &r.pair[1],
                    None,
                    None,
                    Some(a),
                    Some(b),
                );

                // TODO: квотинг dexA->dexB со slippage=slip_bps и проверкой min_profit_bps
            }
        }

        // -------- Треугольники
        for tri in &client.cfg.triangles {
            if self.skip_pair_by_risk(&client.cfg, &tri[0], &tri[1])
                || self.skip_pair_by_risk(&client.cfg, &tri[1], &tri[2])
            {
                continue;
            }

            METRIC_ROUTES_SCANNED.inc();
            let _a = addr_of(&client.cfg, &tri[0])?;
            let _b = addr_of(&client.cfg, &tri[1])?;
            let _c = addr_of(&client.cfg, &tri[2])?;
            // TODO: котировка A→B→C→A
        }

        // -------- Демонстрационные вызовы Executor (если он есть)
        let mut any_success = false;

        if let Some(exec) = self.executors.get(&client.cfg.chain_id) {
            let route_calldata: Bytes = encode_route_calldata(&[], U256::zero(), U256::zero())?;

            // 1) simulate — безопасный staticcall
            let _ = exec.simulate(route_calldata.clone()).await;

            // 2) execute — включается только по ENV (никаких случайных транзакций)
            if std::env::var("DRY_RUN_EXECUTE").is_ok() {
                if let Ok(_tx) = exec.execute(route_calldata.clone(), U256::zero()).await {
                    METRIC_TX_SENT.inc();
                    METRIC_PROFITABLE_FOUND.inc();
                    any_success = true;
                }
            }

            // 3) execute_with_opts — тоже только по ENV
            if std::env::var("DRY_RUN_EXECUTE_OPTS").is_ok() {
                let opts = TxOpts {
                    private: false,
                    gas_jitter: Some(crate::mev::GasJitterCfg {
                        jitter_bps: 50,
                        max_fee_multiplier: 1.2,
                        priority_fee_gwei: 2,
                    }),
                    gas_limit: Some(200_000),
                    // 1 gwei = 10^9 wei
                    legacy_gas_price: Some(u256_from_decimals(1.0, 9)),
                    max_fee_per_gas: None,
                    max_priority_fee_per_gas: None,
                    private_relay: None,
                };
                if let Ok(_tx) = exec
                    .execute_with_opts(route_calldata.clone(), U256::zero(), opts)
                    .await
                {
                    METRIC_TX_SENT.inc();
                    any_success = true;
                }
            }
        }

        // ---- обновляем счётчик серий ----
        if any_success {
            self.pnl.on_success();
        } else {
            self.pnl.on_loss();
        }

        Ok(())
    }
}

// ===== helpers =====

fn addr_of(n: &Network, sym: &str) -> Result<Address> {
    let t = n
        .tokens
        .get(sym)
        .ok_or_else(|| anyhow!("token not found: {sym}"))?;
    parse_addr(&t.address).map_err(|e| anyhow!(e))
}

#[derive(Clone, Debug)]
struct PnLTracker {
    consec_losses: u32,
    last_loss_ts: Option<Instant>,
}
impl PnLTracker {
    fn new() -> Self {
        Self {
            consec_losses: 0,
            last_loss_ts: None,
        }
    }
    fn on_success(&mut self) {
        self.consec_losses = 0;
        self.last_loss_ts = None;
    }
    fn on_loss(&mut self) {
        self.consec_losses = self.consec_losses.saturating_add(1);
        self.last_loss_ts = Some(Instant::now());
    }
    fn should_cooldown(&self, cooldown_sec: u64) -> bool {
        if self.consec_losses == 0 {
            return false;
        }
        self.last_loss_ts
            .map(|ts| ts.elapsed() < Duration::from_secs(cooldown_sec))
            .unwrap_or(false)
    }
}

impl StrategyEngine {
    // Проверка "чёрного списка" токенов
    fn skip_pair_by_risk(&self, net: &Network, a_sym: &str, b_sym: &str) -> bool {
        let bl = &self.cfg.global.risk.blacklist_tokens;
        let has_black = |sym: &str| {
            net.tokens
                .get(sym)
                .map(|t| bl.iter().any(|x| x.eq_ignore_ascii_case(&t.address)))
                .unwrap_or(false)
        };
        if has_black(a_sym) || has_black(b_sym) {
            tracing::warn!("skip pair {}-{}: blacklisted token", a_sym, b_sym);
            return true;
        }
        false
    }

    // Эвристика USD-ликвидности
    #[allow(clippy::too_many_arguments)]
    fn meets_min_liquidity_hint(
        &self,
        net: &Network,
        a_sym: &str,
        b_sym: &str,
        reserve0: Option<U256>,
        reserve1: Option<U256>,
        token0: Option<Address>,
        token1: Option<Address>,
    ) -> bool {
        let min_usd = self.cfg.global.risk.min_liquidity_usd as f64;

        // стейблы задаём в конфиге по СИМВОЛАМ (USDC, USDT, DAI, ...)
        let stable_syms: Vec<String> = self
            .cfg
            .global
            .risk
            .stables
            .iter()
            .map(|s| s.to_uppercase())
            .collect();

        let (r0, r1, t0, t1) = match (reserve0, reserve1, token0, token1) {
            (Some(r0), Some(r1), Some(t0), Some(t1)) => (r0, r1, t0, t1),
            _ => return true, // нет данных — пропускаем проверку
        };

        // адрес -> является ли стейблом по символу
        let is_stable_addr = |addr: &Address| {
            net.tokens.iter().any(|(sym, tk)| {
                parse_addr(&tk.address)
                    .ok()
                    .map(|a| a == *addr && stable_syms.iter().any(|s| s == sym))
                    .unwrap_or(false)
            })
        };

        let usd = if is_stable_addr(&t0) {
            (r0.as_u128() as f64) / 1e18
        } else if is_stable_addr(&t1) {
            (r1.as_u128() as f64) / 1e18
        } else {
            return true;
        };

        if usd < min_usd {
            tracing::warn!(
                "skip pair {}-{}: low liquidity ${:.2} < {}",
                a_sym,
                b_sym,
                usd,
                min_usd
            );
            return false;
        }
        true
    }
}

// Создаёт SignerMiddleware<Provider<Http>, LocalWallet> для указанной сети.
// Ключ берём из ENV: PRIVATE_KEY_<chain_id> или PRIVATE_KEY.
fn signer_middleware_for_chain(
    provider: Arc<Provider<Http>>,
    chain_id: u64,
) -> Result<Arc<SignerMiddleware<Provider<Http>, LocalWallet>>> {
    let pk_env_specific = format!("PRIVATE_KEY_{}", chain_id);
    let pk = std::env::var(&pk_env_specific)
        .or_else(|_| std::env::var("PRIVATE_KEY"))
        .map_err(|_| anyhow!("PRIVATE_KEY (или {pk_env_specific}) не задан"))?;

    let wallet: LocalWallet = pk.parse()?;
    let wallet = wallet.with_chain_id(chain_id);
    let sm = SignerMiddleware::new(provider.as_ref().clone(), wallet);
    Ok(Arc::new(sm))
}
