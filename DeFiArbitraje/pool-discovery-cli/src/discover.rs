use crate::config::{Config, Network, DexConfig};
use anyhow::{Result, anyhow};
use ethers::abi::Abi;
use ethers::contract::Contract;
use ethers::providers::{Provider, Http};
use ethers::types::{Address, U256};
use futures::stream::{StreamExt, FuturesUnordered};
use itertools::Itertools;
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use std::collections::HashMap;
use tracing::{info, warn};

#[derive(Debug, Serialize, Deserialize)]
pub struct Output {
    pub generated_at: String,
    pub networks: Vec<OutNetwork>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OutNetwork {
    pub chain_id: u64,
    pub name: String,
    pub dexes: Vec<OutDex>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OutDex {
    #[serde(rename="v2")]
    V2 { name: String, factory: String, pairs: Vec<OutV2Pair> },
    #[serde(rename="v3")]
    V3 { name: String, factory: String, pools: Vec<OutV3Pool> },
    #[serde(rename="solidly_v2")]
    Solidly { name: String, factory: String, pairs: Vec<OutSolidlyPair> },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OutV2Pair {
    pub pair: [String; 2],
    pub address: String,
    pub token0: String,
    pub token1: String,
    pub reserves0: String,
    pub reserves1: String,
    pub decimals0: u8,
    pub decimals1: u8,
    pub suggested_amount_token0: String,
    pub suggested_amount_token1: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OutSolidlyPair {
    pub pair: [String; 2],
    pub stable: bool,
    pub address: String,
    pub token0: String,
    pub token1: String,
    pub reserves0: String,
    pub reserves1: String,
    pub decimals0: u8,
    pub decimals1: u8,
    pub suggested_amount_token0: String,
    pub suggested_amount_token1: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OutV3Pool {
    pub pair: [String; 2],
    pub fee: u32,
    pub address: String,
    pub token0: String,
    pub token1: String,
    pub sqrt_price_x96: String,
    pub tick: i32,
    pub liquidity: String,
    pub note: String,
}

pub async fn run_discovery(cfg: Config, _concurrency: usize) -> Result<Output> {
    let mut out_networks = Vec::new();
    for n in cfg.networks {
        let Some(rpc) = n.rpc.first() else { continue };
        let provider = Arc::new(Provider::<Http>::try_from(rpc.clone())?);
        info!(chainId=%n.chainId, "Скан сети");

        let mut out_dexes = Vec::new();
        for d in &n.dexes {
            match d.dex_type.as_str() {
                "v2" => {
                    if let Some(factory) = &d.factory {
                        let pairs = discover_v2(&n, provider.clone(), factory).await?;
                        out_dexes.push(OutDex::V2 { name: d.name.clone(), factory: factory.clone(), pairs });
                    } else {
                        warn!("Пропуск v2 {} — нет factory", d.name);
                    }
                }
                "solidly_v2" => {
                    if let Some(factory) = &d.factory {
                        let pairs = discover_solidly(&n, provider.clone(), factory).await?;
                        out_dexes.push(OutDex::Solidly { name: d.name.clone(), factory: factory.clone(), pairs });
                    } else {
                        warn!("Пропуск solidly {} — нет factory", d.name);
                    }
                }
                "v3" => {
                    if let Some(factory) = &d.factory {
                        let fees = d.feeTiers_bps.clone().unwrap_or(vec![100,500,1000,3000,10000]);
                        let pools = discover_v3(&n, provider.clone(), factory, &fees).await?;
                        out_dexes.push(OutDex::V3 { name: d.name.clone(), factory: factory.clone(), pools });
                    } else {
                        warn!("Пропуск v3 {} — нет factory", d.name);
                    }
                }
                _ => {
                    warn!("Неизвестный тип DEX: {}", d.dex_type);
                }
            }
        }

        out_networks.push(OutNetwork {
            chain_id: n.chainId, name: n.name.clone(), dexes: out_dexes
        });
    }

    Ok(Output {
        generated_at: chrono::Utc::now().to_rfc3339(),
        networks: out_networks,
    })
}

async fn discover_v2(n: &Network, provider: Arc<Provider<Http>>, factory: &String) -> Result<Vec<OutV2Pair>> {
    let abi_factory: Abi = serde_json::from_str(include_str!("../abis/UniswapV2Factory.json"))?;
    let c_factory = Contract::new(parse_addr(factory), abi_factory, provider.clone());

    let mut out = Vec::new();
    for [a_sym, b_sym] in n.pairs.iter().cloned() {
        let t_a = n.tokens.get(&a_sym).ok_or_else(|| anyhow!("token {} not found", a_sym))?.address.clone();
        let t_b = n.tokens.get(&b_sym).ok_or_else(|| anyhow!("token {} not found", b_sym))?.address.clone();
        let pair_addr: Address = c_factory.method("getPair", (parse_addr(&t_a), parse_addr(&t_b)))?.call().await?;
        if pair_addr == Address::zero() { continue; }
        let abi_pair: Abi = serde_json::from_str(include_str!("../abis/UniswapV2Pair.json"))?;
        let c_pair = Contract::new(pair_addr, abi_pair, provider.clone());
        let token0: Address = c_pair.method("token0", ())?.call().await?;
        let token1: Address = c_pair.method("token1", ())?.call().await?;
        let (r0, r1, _): (U256, U256, u32) = c_pair.method("getReserves", ())?.call().await?;

        let (dec0, dec1) = token_decimals_by_order(&n.tokens, token0, token1)?;
        let (sug0, sug1) = suggested_from_reserves(r0, r1, 20);

        out.push(OutV2Pair {
            pair: [a_sym, b_sym],
            address: to_hex(pair_addr),
            token0: to_hex(token0),
            token1: to_hex(token1),
            reserves0: r0.to_string(),
            reserves1: r1.to_string(),
            decimals0: dec0,
            decimals1: dec1,
            suggested_amount_token0: sug0.to_string(),
            suggested_amount_token1: sug1.to_string(),
        });
    }
    Ok(out)
}

async fn discover_solidly(n: &Network, provider: Arc<Provider<Http>>, factory: &String) -> Result<Vec<OutSolidlyPair>> {
    let abi_factory: Abi = serde_json::from_str(include_str!("../abis/SolidlyFactory.json"))?;
    let c_factory = Contract::new(parse_addr(factory), abi_factory, provider.clone());

    let mut out = Vec::new();
    for [a_sym, b_sym] in n.pairs.iter().cloned() {
        let t_a = n.tokens.get(&a_sym).ok_or_else(|| anyhow!("token {} not found", a_sym))?.address.clone();
        let t_b = n.tokens.get(&b_sym).ok_or_else(|| anyhow!("token {} not found", b_sym))?.address.clone();

        for &stable in &[false, true] {
            let pair_addr: Address = c_factory.method("getPair", (parse_addr(&t_a), parse_addr(&t_b), stable))?.call().await?;
            if pair_addr == Address::zero() { continue; }
            // используем v2 ABI для token0/token1/getReserves
            let abi_pair_v2: Abi = serde_json::from_str(include_str!("../abis/UniswapV2Pair.json"))?;
            let c_pair_v2 = Contract::new(pair_addr, abi_pair_v2, provider.clone());
            let token0: Address = c_pair_v2.method("token0", ())?.call().await?;
            let token1: Address = c_pair_v2.method("token1", ())?.call().await?;
            let (r0, r1, _): (U256, U256, u32) = c_pair_v2.method("getReserves", ())?.call().await?;

            let (dec0, dec1) = token_decimals_by_order(&n.tokens, token0, token1)?;
            let (sug0, sug1) = suggested_from_reserves(r0, r1, 15);
            out.push(OutSolidlyPair {
                pair: [a_sym.clone(), b_sym.clone()],
                stable,
                address: to_hex(pair_addr),
                token0: to_hex(token0),
                token1: to_hex(token1),
                reserves0: r0.to_string(),
                reserves1: r1.to_string(),
                decimals0: dec0,
                decimals1: dec1,
                suggested_amount_token0: sug0.to_string(),
                suggested_amount_token1: sug1.to_string(),
            });
        }
    }
    Ok(out)
}

async fn discover_v3(n: &Network, provider: Arc<Provider<Http>>, factory: &String, fees: &Vec<u32>) -> Result<Vec<OutV3Pool>> {
    let abi_factory: Abi = serde_json::from_str(include_str!("../abis/UniswapV3Factory.json"))?;
    let c_factory = Contract::new(parse_addr(factory), abi_factory, provider.clone());

    let mut out = Vec::new();
    for [a_sym, b_sym] in n.pairs.iter().cloned() {
        let t_a = n.tokens.get(&a_sym).ok_or_else(|| anyhow!("token {} not found", a_sym))?.address.clone();
        let t_b = n.tokens.get(&b_sym).ok_or_else(|| anyhow!("token {} not found", b_sym))?.address.clone();
        for fee in fees {
            let pool: Address = c_factory.method("getPool", (parse_addr(&t_a), parse_addr(&t_b), *fee))?.call().await?;
            if pool == Address::zero() { continue; }
            let abi_pool: Abi = serde_json::from_str(include_str!("../abis/UniswapV3Pool.json"))?;
            let c_pool = Contract::new(pool, abi_pool, provider.clone());
            let (spx96, tick, _oi, _oc, _ocn, _fp, _unlocked): (U256, i32, u16, u16, u16, u8, bool) = c_pool.method("slot0", ())?.call().await?;
            let liq: U256 = c_pool.method("liquidity", ())?.call().await?;
            let t0: Address = c_pool.method("token0", ())?.call().await?;
            let t1: Address = c_pool.method("token1", ())?.call().await?;
            out.push(OutV3Pool {
                pair: [a_sym.clone(), b_sym.clone()],
                fee: *fee,
                address: to_hex(pool),
                token0: to_hex(t0),
                token1: to_hex(t1),
                sqrt_price_x96: spx96.to_string(),
                tick,
                liquidity: liq.to_string(),
                note: "V3: нет getReserves; используйте liquidity+slot0",
            });
        }
    }
    Ok(out)
}

fn parse_addr(s: &str) -> Address {
    s.parse::<Address>().expect("bad address")
}

fn to_hex(a: Address) -> String {
    format!("{:#x}", a)
}

fn token_decimals_by_order(tokens: &std::collections::HashMap<String, crate::config::Token>, t0: Address, t1: Address) -> anyhow::Result<(u8,u8)> {
    let mut dec0 = None;
    let mut dec1 = None;
    for (_sym, t) in tokens {
        let addr: Address = parse_addr(&t.address);
        if addr == t0 { dec0 = Some(t.decimals); }
        if addr == t1 { dec1 = Some(t.decimals); }
    }
    Ok((dec0.ok_or_else(|| anyhow::anyhow!("decimals0 not found"))?, dec1.ok_or_else(|| anyhow::anyhow!("decimals1 not found"))?))
}

fn suggested_from_reserves(r0: U256, r1: U256, bps: u32) -> (U256, U256) {
    let minr = if r0 < r1 { r0 } else { r1 };
    let amt = minr * U256::from(bps) / U256::from(10_000u64);
    (amt, amt)
}
