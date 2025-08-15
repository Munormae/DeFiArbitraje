use anyhow::Result;
use ethers::types::U256;
use tracing::warn;
use rand::Rng as _;
use rand::rng;

#[derive(Clone, Debug, Default)]
pub struct GasJitterCfg {
    /// +/- bps вокруг исходного значения
    pub jitter_bps: u32,
    /// множитель к basefee для max_fee_per_gas
    pub max_fee_multiplier: f64,
    /// целевая приоритетная комиссия (gwei), может быть заджиттерена
    pub priority_fee_gwei: u64,
}

#[inline]
pub fn jitter_value_bps(value: U256, bps: u32) -> U256 {
    if bps == 0 {
        return value;
    }
    let mut r = rng();
    let up = r.random::<bool>();
    let j = r.random_range(0..=bps as u64);
    let delta = value * U256::from(j) / U256::from(10_000u64);
    if up {
        value + delta
    } else {
        value.saturating_sub(delta)
    }
}

#[inline]
pub fn jitter_u64_bps(v: u64, bps: u32) -> u64 {
    if bps == 0 {
        return v;
    }
    let mut r = rng();
    let up = r.random::<bool>();
    let j = r.random_range(0..=bps as u64);
    let delta = v.saturating_mul(j) / 10_000;
    if up {
        v.saturating_add(delta)
    } else {
        v.saturating_sub(delta)
    }
}

#[derive(Clone, Debug)]
pub struct PrivateRelay {
    pub name: String,
    pub endpoints: Vec<String>,
}

impl PrivateRelay {
    pub fn new(name: &str, endpoints: Vec<String>) -> Self {
        Self { name: name.to_string(), endpoints }
    }

    pub async fn send_raw_tx(&self, _raw_tx: &str) -> Result<()> {
        // TODO: flashbots_sendRawTransaction / eth_sendPrivateRawTransaction
        warn!(
            "PrivateRelay[{}]: заглушка private-tx; используйте публичный send() как fallback",
            self.name
        );
        Ok(())
    }
}