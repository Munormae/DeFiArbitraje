use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::{env, fs};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub version: String,
    pub created_at: String,
    #[serde(default)]
    pub notes: Option<String>,

    pub global: Global,
    pub networks: Vec<Network>,
    pub strategies: Vec<Strategy>,
    pub routing: Routing,
    pub safety: Safety,
    pub telemetry: Telemetry,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let data = fs::read_to_string(path)?;
        let mut c: Self = serde_json::from_str(&data)?;
        c.expand_env_in_rpcs();
        c.normalize_addresses(); // адреса -> lower, пары/треугольники/маршруты -> UPPER
        c.normalize_token_keys(); // КЛЮЧИ tokens -> UPPERCASE
        c.validate()?;
        Ok(c)
    }

    /// Подстановка ${ENV_VAR} и $ENV_VAR в полях Network.rpc
    fn expand_env_in_rpcs(&mut self) {
        fn expand(s: &str) -> String {
            let mut out = String::new();
            let bytes = s.as_bytes();
            let mut i = 0usize;
            while i < bytes.len() {
                if bytes[i] == b'$' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                        if let Some(end) = s[i + 2..].find('}') {
                            let key = &s[i + 2..i + 2 + end];
                            let val = env::var(key).unwrap_or_default();
                            out.push_str(&val);
                            i += 3 + end;
                            continue;
                        }
                    } else {
                        let rest = &s[i + 1..];
                        let mut j = 0usize;
                        while j < rest.len()
                            && (rest.as_bytes()[j].is_ascii_alphanumeric()
                                || rest.as_bytes()[j] == b'_')
                        {
                            j += 1;
                        }
                        let key = &rest[..j];
                        let val = env::var(key).unwrap_or_default();
                        out.push_str(&val);
                        i += 1 + j;
                        continue;
                    }
                }
                out.push(bytes[i] as char);
                i += 1;
            }
            out
        }
        for n in &mut self.networks {
            n.rpc = n.rpc.iter().map(|u| expand(u)).collect();
        }
    }

    /// Нормализация адресов (нижний регистр) и символов пар/треугольников/маршрутов (UPPERCASE)
    fn normalize_addresses(&mut self) {
        for net in &mut self.networks {
            for t in net.tokens.values_mut() {
                t.address = t.address.trim().to_lowercase();
            }
            if let Some(routes) = net.routes_cross_dex.as_mut() {
                for r in routes.iter_mut() {
                    for s in r.pair.iter_mut() {
                        *s = s.trim().to_uppercase();
                    }
                }
            }
            for p in net.pairs.iter_mut() {
                for s in p.iter_mut() {
                    *s = s.trim().to_uppercase();
                }
            }
            for tri in net.triangles.iter_mut() {
                for s in tri.iter_mut() {
                    *s = s.trim().to_uppercase();
                }
            }
        }
        if !self.global.risk.permit2.is_empty() {
            self.global.risk.permit2 = self.global.risk.permit2.trim().to_lowercase();
        }
    }

    /// Нормализация КЛЮЧЕЙ токенов: "usdc"/"USDbC" → "USDC"/"USDBC"
    fn normalize_token_keys(&mut self) {
        for net in &mut self.networks {
            let mut new_map: HashMap<String, Token> = HashMap::with_capacity(net.tokens.len());
            for (sym, mut t) in std::mem::take(&mut net.tokens) {
                // адрес уже нормализован раньше; на всякий нажмём ещё раз
                t.address = t.address.trim().to_lowercase();
                let key = sym.trim().to_uppercase();
                new_map.insert(key, t);
            }
            net.tokens = new_map;
        }
        // ещё нормализуем список стейблов в global.risk
        for s in &mut self.global.risk.stables {
            *s = s.trim().to_uppercase();
        }
    }

    /// Базовая валидация конфигурации
    pub fn validate(&self) -> Result<()> {
        if self.networks.is_empty() {
            return Err(anyhow!("config.networks is empty"));
        }

        // стратегии: уникальные имена + лимиты
        let mut names = HashSet::new();
        for s in &self.strategies {
            if !names.insert(s.name.clone()) {
                return Err(anyhow!("duplicate strategy name: {}", s.name));
            }
            if s.slippage_bps > 5_000 {
                return Err(anyhow!(
                    "strategy '{}' slippage_bps > 5000 ({} bps)",
                    s.name,
                    s.slippage_bps
                ));
            }
        }

        // сети
        for n in &self.networks {
            if n.id.trim().is_empty() || n.name.trim().is_empty() {
                return Err(anyhow!("network id/name must be non-empty"));
            }
            if n.rpc.is_empty() {
                return Err(anyhow!("network '{}' has no rpc endpoints", n.name));
            }
            // токены
            for (sym, t) in &n.tokens {
                if !is_hex_addr(&t.address) {
                    return Err(anyhow!(
                        "network '{}': token {} has invalid address '{}'",
                        n.name,
                        sym,
                        t.address
                    ));
                }
                if t.decimals > 30 {
                    return Err(anyhow!(
                        "network '{}': token {} decimals looks wrong: {}",
                        n.name,
                        sym,
                        t.decimals
                    ));
                }
            }
            // пары
            for [a, b] in &n.pairs {
                if !n.tokens.contains_key(a) || !n.tokens.contains_key(b) {
                    return Err(anyhow!(
                        "network '{}': pair [{}, {}] refers to unknown tokens",
                        n.name,
                        a,
                        b
                    ));
                }
            }
            // треугольники
            for [a, b, c] in &n.triangles {
                for sym in [a, b, c] {
                    if !n.tokens.contains_key(sym) {
                        return Err(anyhow!(
                            "network '{}': triangle contains unknown token '{}'",
                            n.name,
                            sym
                        ));
                    }
                }
            }
            // DEX конфиги
            for d in &n.dexes {
                if d.dex_type.trim().is_empty() || d.name.trim().is_empty() {
                    return Err(anyhow!(
                        "network '{}': dex entry has empty name/type",
                        n.name
                    ));
                }

                // Разрешаем распространённые тировки для v3/альгебры:
                // - Uniswap-подобные: 100, 500, 3000, 10000
                // - Pancake/Algebra и др.: добавляем 250 и 1000
                if d.dex_type.eq_ignore_ascii_case("v3")
                    || d.dex_type.eq_ignore_ascii_case("v3_algebra")
                {
                    if let Some(fees) = &d.fee_tiers_bps {
                        const KNOWN_V3_FEES: [u32; 6] = [100, 250, 500, 1000, 3000, 10_000];
                        for f in fees {
                            if !KNOWN_V3_FEES.contains(f) {
                                tracing::warn!(
                                    "network '{}': dex '{}' has uncommon fee tier: {} bps",
                                    n.name,
                                    d.name,
                                    f
                                );
                                // ВАЖНО: не валим конфиг на «нестандартных» тирах
                            }
                        }
                    }
                }
            }
        }

        // глобальные лимиты
        if self.global.quote.slippage_bps_default > 5_000 {
            return Err(anyhow!(
                "global.quote.slippage_bps_default too large (>5000 bps)"
            ));
        }
        if !self.global.risk.permit2.is_empty() && !is_hex_addr(&self.global.risk.permit2) {
            return Err(anyhow!("global.risk.permit2 must be 0x-address or empty"));
        }

        Ok(())
    }

    /// Строгая валидация для прод-стека (>=5 сетей)
    pub fn validate_strict(&self) -> Result<()> {
        self.validate()?;
        if self.networks.len() < 5 {
            return Err(anyhow!(
                "strict: expected at least 5 networks, got {}",
                self.networks.len()
            ));
        }
        Ok(())
    }

    // ===== Утилиты =====

    pub fn network(&self, id_or_name: &str) -> Option<&Network> {
        self.networks.iter().find(|n| {
            n.id.eq_ignore_ascii_case(id_or_name) || n.name.eq_ignore_ascii_case(id_or_name)
        })
    }

    pub fn token_addr<'a>(&self, net: &'a Network, symbol: &str) -> Option<&'a str> {
        let key = symbol.to_uppercase();
        net.tokens.get(&key).map(|t| t.address.as_str())
    }

    pub fn primary_rpc<'a>(&self, net: &'a Network) -> &'a str {
        &net.rpc[0]
    }
}

// ================== Глобальные секции ==================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Global {
    pub quote: Quote,
    pub risk: Risk,
    pub mev: Mev,
    pub flashloan: Flashloan,
    pub execution: Execution,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Quote {
    #[serde(default)]
    pub method_order: Vec<String>,
    #[serde(default)]
    pub tick_liquidity_sample: Option<u32>,
    #[serde(default = "default_slippage_bps")]
    pub slippage_bps_default: u32,
    #[serde(default = "default_gas_mode")]
    pub gas_price_mode: String,
    #[serde(default = "default_deadline")]
    pub deadline_seconds: u32,
}
fn default_slippage_bps() -> u32 {
    50
}
fn default_gas_mode() -> String {
    "eip1559".to_string()
}
fn default_deadline() -> u32 {
    120
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Risk {
    #[serde(default)]
    pub min_liquidity_usd: u64,
    #[serde(default)]
    pub max_price_impact_bps: u32,
    #[serde(default)]
    pub stables: Vec<String>,
    #[serde(default)]
    pub blacklist_tokens: Vec<String>,
    #[serde(default)]
    pub fee_on_transfer_block: bool,
    #[serde(default)]
    pub rebase_token_block: bool,
    #[serde(default)]
    pub permit2: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mev {
    #[serde(default)]
    pub private_tx: bool,
    #[serde(default)]
    pub nonce_randomize: bool,
    #[serde(default)]
    pub max_backrun_blocks: u32,
    #[serde(default)]
    pub builders: HashMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Flashloan {
    #[serde(default)]
    pub use_uniswap_v3_flash: bool,
    #[serde(default)]
    pub use_balancer_flash: bool,
    #[serde(default)]
    pub min_profit_usd: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Execution {
    #[serde(default)]
    pub block_range_back: u32,
    #[serde(default = "default_poll_ms")]
    pub poll_interval_ms: u32,
    #[serde(default = "default_priority_fee")]
    pub priority_fee: String, // "auto" | "2gwei"
    #[serde(default = "default_max_conc")]
    pub max_concurrent_txs: u32,
    #[serde(default = "default_revert_retry")]
    pub revert_retry: u32,
    #[serde(default)]
    pub approve_spend_on_start: bool,
    #[serde(default)]
    pub auto_scale_notional: bool,
}
fn default_poll_ms() -> u32 {
    1500
}
fn default_priority_fee() -> String {
    "auto".to_string()
}
fn default_max_conc() -> u32 {
    4
}
fn default_revert_retry() -> u32 {
    1
}

// ================== Сеть/DEX/Маршруты ==================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Network {
    pub id: String,
    pub name: String,
    // Rust-поле в snake_case; JSON может быть "chainId" или "chain_id"
    #[serde(rename = "chainId", alias = "chain_id")]
    pub chain_id: u64,
    pub native_symbol: String,
    pub rpc: Vec<String>,
    #[serde(default, rename = "nativeUsdHint", alias = "native_usd_hint")]
    pub native_usd_hint: Option<f64>,
    #[serde(default)]
    pub explorer: Option<String>,
    #[serde(default)]
    pub tokens: HashMap<String, Token>,
    #[serde(default)]
    pub dexes: Vec<DexConfig>,
    #[serde(default)]
    pub pairs: Vec<[String; 2]>,
    #[serde(default)]
    pub triangles: Vec<[String; 3]>,
    #[serde(default)]
    pub routes_cross_dex: Option<Vec<RouteDex>>,
    #[serde(default)]
    pub strategy_overrides: Option<StrategyOverrides>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Token {
    pub address: String,
    pub decimals: u8,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DexConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub dex_type: String,
    #[serde(default)]
    pub factory: Option<String>,
    #[serde(default)]
    pub router: Option<String>,

    // camelCase поля → snake_case с сохранением JSON-имён
    #[serde(default, rename = "smartRouter", alias = "smart_router")]
    pub smart_router: Option<String>,
    #[serde(default, rename = "swapRouter02", alias = "swap_router02")]
    pub swap_router02: Option<String>,
    #[serde(default, rename = "universalRouter", alias = "universal_router")]
    pub universal_router: Option<String>,
    #[serde(default, rename = "quoterV2_hint", alias = "quoter_v2_hint")]
    pub quoter_v2_hint: Option<bool>,
    #[serde(default, rename = "feeTiers_bps", alias = "fee_tiers_bps")]
    pub fee_tiers_bps: Option<Vec<u32>>,
    #[serde(default, rename = "stablePools", alias = "stable_pools")]
    pub stable_pools: Option<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteDex {
    pub pair: [String; 2],
    pub dexes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategyOverrides {
    #[serde(default)]
    pub min_profit_bps: Option<u32>,
    #[serde(default)]
    pub slippage_bps: Option<u32>,
}

// ================== Стратегии/Маршрутизация ==================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Strategy {
    pub name: String,
    pub description: String,
    pub min_profit_bps: u32,
    pub slippage_bps: u32,
    pub gas_limit: u64,
    #[serde(default)]
    pub max_notional_usd: Option<f64>,
    #[serde(default)]
    pub use_flash: Option<bool>,
    #[serde(default)]
    pub max_route_hops: Option<u32>,
    #[serde(default)]
    pub search_space: Option<String>,
    #[serde(default)]
    pub mev: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Routing {
    pub price_simulation: PriceSim,
    pub route_templates: Vec<RouteTemplate>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PriceSim {
    #[serde(default)]
    pub slippage_checks: bool,
    #[serde(default)]
    pub virtual_reserves_boost_bps: u32,
    #[serde(default)]
    pub max_split_paths: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteTemplate {
    #[serde(rename = "type")]
    pub typ: String, // "single" | "multi" | "triangle"
    pub max_hops: u32,
    #[serde(default)]
    pub legs: Option<u32>,
}

// ================== Безопасность/Телеметрия ==================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Safety {
    #[serde(default)]
    pub allow_revert_on_no_profit: bool,
    #[serde(default)]
    pub halt_on_large_slippage_bps: u32,
    #[serde(default)]
    pub halt_on_volatility_index: f64,
    pub circuit_breaker: CircuitBreaker,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CircuitBreaker {
    pub max_losses_in_row: u32,
    pub cooldown_sec: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Telemetry {
    pub prometheus: PrometheusCfg,
    pub logs: LogsCfg,
    pub alerts: AlertsCfg,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrometheusCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_prom_port")]
    pub port: u16,
}
fn default_prom_port() -> u16 {
    9090
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogsCfg {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub json: bool,
}
fn default_log_level() -> String {
    "info".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlertsCfg {
    #[serde(default)]
    pub email: bool,
    #[serde(default)]
    pub tg_bot: bool,
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub tg_token: Option<String>,
    #[serde(default)]
    pub tg_chat_id: Option<String>,
}

// ================== Helpers ==================

fn is_hex_addr(s: &str) -> bool {
    let s = s.trim();
    s.len() == 42 && s.starts_with("0x") && s[2..].chars().all(|c| c.is_ascii_hexdigit())
}
