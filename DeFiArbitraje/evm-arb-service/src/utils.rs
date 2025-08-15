use ethers::types::{U256, Address};
use std::str::FromStr;

/// Парсит Ethereum-адрес из строки.
/// Возвращает Result вместо panic.
pub fn parse_addr(s: &str) -> Result<Address, String> {
    Address::from_str(s).map_err(|e| format!("Invalid address `{s}`: {e}"))
}

/// Преобразует число с плавающей точкой в U256 с учётом decimals.
/// Округляет вниз (floor), чтобы избежать переполнения и ошибок при конвертации.
pub fn u256_from_decimals(amount: f64, decimals: u8) -> U256 {
    let factor = 10u128.pow(decimals as u32);
    let v = (amount * factor as f64).floor() as u128;
    U256::from(v)
}

/// Переводит число в долях процента (basis points) в обычный коэффициент.
/// Например: 50 bps → 0.005
pub fn bps(v: f64) -> f64 {
    v / 10_000.0
}