use anyhow::{anyhow, Result};
use ethers::prelude::*;
use ethers::types::{Address, U256};
use std::sync::Arc;

// ---------- Strongly-typed ABI ----------
abigen!(
    IUniswapV2Pair,
    r#"[
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast)
        function token0() external view returns (address)
        function token1() external view returns (address)
    ]"#
);

abigen!(
    IUniswapV2Factory,
    r#"[ function getPair(address tokenA, address tokenB) external view returns (address) ]"#
);

abigen!(
    IUniswapV3Factory,
    r#"[ function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address) ]"#
);

abigen!(
    IUniswapV3Pool,
    r#"[
        function slot0() external view returns (uint160 sqrtPriceX96,int24 tick,uint16 observationIndex,uint16 observationCardinality,uint16 observationCardinalityNext,uint8 feeProtocol,bool unlocked)
        function liquidity() external view returns (uint128)
        function token0() external view returns (address)
        function token1() external view returns (address)
        function fee() external view returns (uint24)
    ]"#
);

abigen!(
    IQuoterV2,
    r#"[ function quoteExactInputSingle(address tokenIn,address tokenOut,uint24 fee,uint256 amountIn,uint160 sqrtPriceLimitX96) external returns (uint256 amountOut,uint160 sqrtPriceX96After,uint32 initializedTicksCrossed,uint256 gasEstimate) ]"#
);

abigen!(
    ISolidlyFactory,
    r#"[ function getPair(address tokenA,address tokenB,bool stable) external view returns (address) ]"#
);

abigen!(
    ISolidlyPair,
    r#"[ function getAmountOut(uint256 amountIn,address tokenIn) external view returns (uint256) ]"#
);

// ---------- V2 ----------
pub struct V2Pair {
    pub pair: Address,
}

impl V2Pair {
    pub async fn get_reserves<M: Middleware + 'static>(&self, mw: Arc<M>) -> Result<(U256, U256)> {
        let c = IUniswapV2Pair::new(self.pair, mw);
        let (r0, r1, _ts) = c.get_reserves().call().await?;
        Ok((U256::from(r0), U256::from(r1)))
    }
}

/// Константный продукт с комиссией fee_bps (например, 30 = 0.30%)
pub fn amount_out_v2(amount_in: U256, reserve_in: U256, reserve_out: U256, fee_bps: u32) -> U256 {
    if amount_in.is_zero() || reserve_in.is_zero() || reserve_out.is_zero() {
        return U256::zero();
    }
    let fee = U256::from(10_000u64 - fee_bps as u64);
    let amount_in_with_fee = amount_in * fee;
    let numerator = amount_in_with_fee * reserve_out;
    let denominator = reserve_in * U256::from(10_000u64) + amount_in_with_fee;
    numerator / denominator
}

pub async fn v2_get_pair<M: Middleware + 'static>(
    mw: Arc<M>,
    factory: Address,
    a: Address,
    b: Address,
) -> Result<Address> {
    let f = IUniswapV2Factory::new(factory, mw);
    Ok(f.get_pair(a, b).call().await?)
}

// ---------- V3 ----------
pub async fn v3_get_pool<M: Middleware + 'static>(
    mw: Arc<M>,
    factory: Address,
    a: Address,
    b: Address,
    fee: u32,
) -> Result<Address> {
    let f = IUniswapV3Factory::new(factory, mw);
    Ok(f.get_pool(a, b, fee).call().await?)
}

/// slot0() + liquidity(); возвращает (sqrtPriceX96, tick, liquidity)
pub async fn v3_slot0_liquidity<M: Middleware + 'static>(
    mw: Arc<M>,
    pool: Address,
) -> Result<(U256, i32, U256)> {
    let p = IUniswapV3Pool::new(pool, mw);
    let (sqrt_price_x96, tick, ..) = p.slot_0().call().await?;
    let liq = U256::from(p.liquidity().call().await?);
    Ok((U256::from(sqrt_price_x96), tick, liq))
}

/// Квота через QuoterV2
pub async fn v3_quote_exact_input_single<M: Middleware + 'static>(
    mw: Arc<M>,
    quoter_v2: Address,
    token_in: Address,
    token_out: Address,
    fee: u32,
    amount_in: U256,
) -> Result<(U256, U256)> {
    let q = IQuoterV2::new(quoter_v2, mw);
    let (amount_out, sqrt_after, ..) =
        q.quote_exact_input_single(token_in, token_out, fee, amount_in, U256::zero())
            .call()
            .await?;
    Ok((amount_out, U256::from(sqrt_after)))
}

/// Перевод sqrtPriceX96 → цена (tokenOut per tokenIn)
pub fn v3_price_from_sqrt_x96(
    sqrt_price_x96: U256,
    decimals_in: u8,
    decimals_out: u8,
) -> f64 {
    let num = sqrt_price_x96.full_mul(sqrt_price_x96);
    let hi = (num >> 192).as_u128();
    let base = hi as f64;
    let scale = 10f64.powi(decimals_out as i32 - decimals_in as i32);
    base * scale
}

/// minOut по bps
pub fn min_out_bps(quoted_out: U256, slippage_bps: u32) -> U256 {
    let bps = U256::from(10_000u64);
    quoted_out * (bps - U256::from(slippage_bps)) / bps
}

// ---------- Solidly ----------
pub async fn solidly_get_pair<M: Middleware + 'static>(
    mw: Arc<M>,
    factory: Address,
    a: Address,
    b: Address,
    stable: bool,
) -> Result<Address> {
    let f = ISolidlyFactory::new(factory, mw);
    Ok(f.get_pair(a, b, stable).call().await?)
}

pub async fn solidly_pair_get_amount_out<M: Middleware + 'static>(
    mw: Arc<M>,
    pair: Address,
    amount_in: U256,
    token_in: Address,
) -> Result<U256> {
    let p = ISolidlyPair::new(pair, mw);
    Ok(p.get_amount_out(amount_in, token_in).call().await?)
}

// ---------- Утилиты ----------
pub async fn v2_pair_tokens<M: Middleware + 'static>(
    mw: Arc<M>,
    pair: Address,
) -> Result<(Address, Address)> {
    let c = IUniswapV2Pair::new(pair, mw);
    let t0 = c.token_0().call().await?;
    let t1 = c.token_1().call().await?;
    Ok((t0, t1))
}

pub async fn v3_pool_meta<M: Middleware + 'static>(
    mw: Arc<M>,
    pool: Address,
) -> Result<(Address, Address, u32)> {
    let p = IUniswapV3Pool::new(pool, mw);
    let t0 = p.token_0().call().await?;
    let t1 = p.token_1().call().await?;
    let fee = p.fee().call().await?;
    Ok((t0, t1, fee))
}

/// Проверка на нулевой адрес
pub fn ensure_not_zero(addr: Address, what: &str) -> Result<Address> {
    if addr == Address::zero() {
        return Err(anyhow!("{} returned zero address", what));
    }
    Ok(addr)
}
