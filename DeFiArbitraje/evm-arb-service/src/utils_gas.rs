use anyhow::Result;
use ethers::providers::Middleware;
use ethers::types::{BlockNumber, U256};
use std::cmp::min;
use std::env;
use std::sync::Arc;

/// Get current legacy gas price (wei) taking into account EIP-1559 fields if available
pub async fn current_gas_price_legacy<M>(mw: Arc<M>) -> Result<U256>
where
    M: Middleware + 'static,
    M::Error: 'static,
{
    let tip_gwei: u64 = env::var("GAS_TIP_GWEI")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let tip = U256::from(tip_gwei) * U256::exp10(9);

    if let Some(block) = mw.get_block(BlockNumber::Latest).await? {
        if let Some(base_fee) = block.base_fee_per_gas {
            let base_plus_tip = base_fee + tip;
            if let Ok((max_fee_per_gas, _)) = mw.estimate_eip1559_fees(None).await {
                return Ok(min(max_fee_per_gas, base_plus_tip));
            }
            return Ok(base_plus_tip);
        }
    }

    Ok(mw.get_gas_price().await?)
}

/// Calculate gas cost in native tokens
pub fn gas_cost_native(gas_units: u64, gas_price: U256) -> f64 {
    let price_native = (gas_price.as_u128() as f64) / 1e18f64;
    price_native * gas_units as f64
}

/// Convert native token amount to USD
pub fn gas_cost_usd(native_amount: f64, native_usd: f64) -> f64 {
    native_amount * native_usd
}

