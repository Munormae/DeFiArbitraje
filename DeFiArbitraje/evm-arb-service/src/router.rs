use anyhow::{Result, anyhow};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Address, U256};
use std::sync::Arc;
use tracing::debug;

use crate::calldata::{LegKind, LegQuote};
use crate::config::{DexConfig, Network};
use crate::dex::{
    V2Pair, amount_out_v2, ensure_not_zero, min_out_bps, solidly_get_pair,
    solidly_pair_get_amount_out, v2_get_pair, v2_pair_tokens, v3_get_pool,
    v3_quote_exact_input_single,
};
use crate::utils::parse_addr;

/// Результат квотинга маршрута
pub struct QuoteResult {
    pub amount_in: U256,
    pub amount_out: U256,
    pub gas_estimate: u64,
    pub legs: Vec<LegQuote>,
    pub pnl_usd: f64,
}

// helper: проверка, является ли символ native-токеном (ETH/WETH и т.п.)
fn is_native_symbol(net: &Network, sym: &str) -> bool {
    let s = sym.to_uppercase();
    let native = net.native_symbol.to_uppercase();
    s == native || s == format!("W{}", native)
}

fn addr_of(net: &Network, sym: &str) -> Result<Address> {
    let t = net
        .tokens
        .get(&sym.to_uppercase())
        .ok_or_else(|| anyhow!("token not found: {sym}"))?;
    parse_addr(&t.address).map_err(|e| anyhow!(e))
}

fn decimals_of(net: &Network, sym: &str) -> u8 {
    net.tokens
        .get(&sym.to_uppercase())
        .map(|t| t.decimals)
        .unwrap_or(18)
}

async fn quote_on_dex(
    client: Arc<Provider<Http>>,
    net: &Network,
    dex: &DexConfig,
    token_in_sym: &str,
    token_out_sym: &str,
    amount_in: U256,
) -> Result<Option<(U256, LegQuote, u64)>> {
    let token_in = addr_of(net, token_in_sym)?;
    let token_out = addr_of(net, token_out_sym)?;

    match dex.dex_type.to_lowercase().as_str() {
        "v2" => {
            let factory = parse_addr(
                dex.factory
                    .as_ref()
                    .ok_or_else(|| anyhow!("v2 factory missing"))?,
            )
            .map_err(|e| anyhow!(e))?;
            let pair_addr = ensure_not_zero(
                v2_get_pair(client.clone(), factory, token_in, token_out).await?,
                "v2_get_pair",
            )?;
            let (t0, _t1) = v2_pair_tokens(client.clone(), pair_addr).await?;
            let (r0, r1) = V2Pair { pair: pair_addr }
                .get_reserves(client.clone())
                .await?;
            let (res_in, res_out) = if token_in == t0 { (r0, r1) } else { (r1, r0) };
            let fee_bps = if dex.name.to_lowercase().contains("pancakev2") {
                25
            } else {
                30
            };
            let out = amount_out_v2(amount_in, res_in, res_out, fee_bps);
            if out.is_zero() {
                return Ok(None);
            }
            let router = parse_addr(
                dex.router
                    .as_ref()
                    .ok_or_else(|| anyhow!("v2 router missing"))?,
            )
            .map_err(|e| anyhow!(e))?;
            let leg = LegQuote {
                kind: LegKind::V2 {
                    router,
                    path: vec![token_in, token_out],
                },
            };
            Ok(Some((out, leg, 110_000)))
        }
        "v3" => {
            let factory = parse_addr(
                dex.factory
                    .as_ref()
                    .ok_or_else(|| anyhow!("v3 factory missing"))?,
            )
            .map_err(|e| anyhow!(e))?;
            let router_str = dex
                .swap_router02
                .as_ref()
                .or(dex.universal_router.as_ref())
                .or(dex.router.as_ref())
                .ok_or_else(|| anyhow!("v3 router missing"))?;
            let router = parse_addr(router_str).map_err(|e| anyhow!(e))?;

            let quoter_addr = if dex.quoter_v2_hint.unwrap_or(false) {
                parse_addr(
                    dex.swap_router02
                        .as_ref()
                        .or(dex.universal_router.as_ref())
                        .ok_or_else(|| anyhow!("v3 quoter unknown"))?,
                )
                .ok()
            } else {
                None
            };

            if quoter_addr.is_none() {
                return Ok(None);
            }
            let quoter = quoter_addr.unwrap();

            let fee_tiers: Vec<u32> = dex.fee_tiers_bps.clone().unwrap_or_else(|| vec![3000]);
            for fee in fee_tiers {
                let pool = v3_get_pool(client.clone(), factory, token_in, token_out, fee).await?;
                if pool == Address::zero() {
                    continue;
                }
                let (out, _) = v3_quote_exact_input_single(
                    client.clone(),
                    quoter,
                    token_in,
                    token_out,
                    fee,
                    amount_in,
                )
                .await?;
                if out.is_zero() {
                    continue;
                }
                let leg = LegQuote {
                    kind: LegKind::V3 {
                        router,
                        token_in,
                        token_out,
                        fee_bps: fee,
                    },
                };
                return Ok(Some((out, leg, 140_000)));
            }
            Ok(None)
        }
        t if t.starts_with("solidly") => {
            let factory = parse_addr(
                dex.factory
                    .as_ref()
                    .ok_or_else(|| anyhow!("solidly factory missing"))?,
            )
            .map_err(|e| anyhow!(e))?;
            let router = parse_addr(
                dex.router
                    .as_ref()
                    .ok_or_else(|| anyhow!("solidly router missing"))?,
            )
            .map_err(|e| anyhow!(e))?;
            // сначала volatile
            let mut stable = false;
            let mut pair_addr =
                solidly_get_pair(client.clone(), factory, token_in, token_out, false).await?;
            if pair_addr == Address::zero() && dex.stable_pools.unwrap_or(false) {
                stable = true;
                pair_addr =
                    solidly_get_pair(client.clone(), factory, token_in, token_out, true).await?;
            }
            if pair_addr == Address::zero() {
                return Ok(None);
            }
            let out =
                solidly_pair_get_amount_out(client.clone(), pair_addr, amount_in, token_in).await?;
            if out.is_zero() {
                return Ok(None);
            }
            let leg = LegQuote {
                kind: LegKind::Solidly {
                    router,
                    pair: pair_addr,
                    stable,
                    token_in,
                },
            };
            Ok(Some((out, leg, 110_000)))
        }
        _ => Ok(None),
    }
}

async fn estimate_pnl_usd(
    client: Arc<Provider<Http>>,
    net: &Network,
    start_sym: &str,
    amount_in: U256,
    amount_out: U256,
    gas_estimate: u64,
) -> Result<f64> {
    let price_hint = match net.native_usd_hint {
        Some(p) => p,
        None => return Ok(0.0),
    };
    let gas_price = client.get_gas_price().await?;
    let gas_price_native = (gas_price.as_u128() as f64) / 1e18f64;
    let gas_cost_native = gas_price_native * gas_estimate as f64;

    let mut profit_native = 0.0f64;
    if is_native_symbol(net, start_sym) {
        let dec = decimals_of(net, start_sym) as i32;
        let diff = if amount_out > amount_in {
            amount_out - amount_in
        } else {
            U256::zero()
        };
        profit_native = (diff.as_u128() as f64) / 10f64.powi(dec);
    }

    let pnl_native = profit_native - gas_cost_native;
    Ok(pnl_native * price_hint)
}

pub async fn quote_cross_dex_pair(
    client: Arc<Provider<Http>>,
    net: &Network,
    pair: (&str, &str),
    dex_a: &DexConfig,
    dex_b: &DexConfig,
    amount_in: U256,
    slip_bps: u32,
) -> Result<Option<QuoteResult>> {
    let (sym_a, sym_b) = pair;
    let mut legs: Vec<LegQuote> = Vec::new();
    let mut gas_total = 0u64;

    let mut amount = amount_in;
    let (out1, leg1, gas1) =
        match quote_on_dex(client.clone(), net, dex_a, sym_a, sym_b, amount).await? {
            Some(v) => v,
            None => return Ok(None),
        };
    legs.push(leg1);
    gas_total += gas1;
    amount = out1;

    let (out2, leg2, gas2) =
        match quote_on_dex(client.clone(), net, dex_b, sym_b, sym_a, amount).await? {
            Some(v) => v,
            None => return Ok(None),
        };
    legs.push(leg2);
    gas_total += gas2;
    amount = out2;

    let gas_estimate = ((gas_total as f64) * 1.15).ceil() as u64;
    let pnl_usd =
        estimate_pnl_usd(client.clone(), net, sym_a, amount_in, amount, gas_estimate).await?;
    let min_out = min_out_bps(amount, slip_bps);
    if min_out <= amount_in {
        return Ok(None);
    }
    if amount <= amount_in {
        return Ok(None);
    }
    debug!(
        "candidate pnl_usd={:.4}, gas={}, legs={}",
        pnl_usd,
        gas_estimate,
        legs.len()
    );

    Ok(Some(QuoteResult {
        amount_in,
        amount_out: amount,
        gas_estimate,
        legs,
        pnl_usd,
    }))
}

pub async fn quote_triangle(
    client: Arc<Provider<Http>>,
    net: &Network,
    tri: (&str, &str, &str),
    preferred_dexes: &[String],
    amount_in: U256,
    slip_bps: u32,
) -> Result<Option<QuoteResult>> {
    let (a, b, c) = tri;
    let mut amount = amount_in;
    let mut legs: Vec<LegQuote> = Vec::new();
    let mut gas_total = 0u64;

    let pairs = [(a, b), (b, c), (c, a)];
    for (tin, tout) in pairs.iter() {
        // составляем порядок dex: preferred сначала
        let mut dex_order: Vec<&DexConfig> = Vec::new();
        for name in preferred_dexes {
            if let Some(d) = net.dexes.iter().find(|d| d.name == *name) {
                dex_order.push(d);
            }
        }
        for d in &net.dexes {
            if !preferred_dexes.iter().any(|n| n == &d.name) {
                dex_order.push(d);
            }
        }
        let mut quoted = None;
        for d in dex_order {
            if let Some(res) = quote_on_dex(client.clone(), net, d, tin, tout, amount).await? {
                quoted = Some((res.0, res.1, res.2));
                break;
            }
        }
        let (out, leg, gas) = match quoted {
            Some(v) => v,
            None => return Ok(None),
        };
        amount = out;
        legs.push(leg);
        gas_total += gas;
    }

    let gas_estimate = ((gas_total as f64) * 1.15).ceil() as u64;
    let pnl_usd = estimate_pnl_usd(client.clone(), net, a, amount_in, amount, gas_estimate).await?;
    let min_out = min_out_bps(amount, slip_bps);
    if min_out <= amount_in {
        return Ok(None);
    }
    if amount <= amount_in {
        return Ok(None);
    }
    debug!(
        "candidate pnl_usd={:.4}, gas={}, legs={}",
        pnl_usd,
        gas_estimate,
        legs.len()
    );

    Ok(Some(QuoteResult {
        amount_in,
        amount_out: amount,
        gas_estimate,
        legs,
        pnl_usd,
    }))
}
