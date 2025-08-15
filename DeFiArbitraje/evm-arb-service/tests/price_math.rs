use pretty_assertions::assert_eq;
use ethers::types::U256;
use DeFiArbitraje::dex::amount_out_v2;
use evm_arb_service::dex::amount_out_v2;

#[test]
fn test_amount_out_v2_basic() {
    let amount_in = U256::from(1000u64);
    let r_in = U256::from(1_000_000u64);
    let r_out = U256::from(1_000_000u64);
    let out = amount_out_v2(amount_in, r_in, r_out, 30);
    assert!(out > U256::zero());
}
