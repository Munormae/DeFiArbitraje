use std::sync::Arc;

use anyhow::Result;
use ethers::prelude::*;
use tracing::{debug, info};

use crate::config::Network;

abigen!(
    IERC20,
    r#"[function allowance(address owner,address spender)view returns(uint256)
                     function approve(address spender,uint256 amount) returns(bool)]"#,
);
abigen!(
    IPermit2,
    r#"[function approve(address token, address spender, uint160 amount, uint48 expiration, uint48 nonce)]"#,
);

pub async fn ensure_approvals<M, S>(
    sm: Arc<SignerMiddleware<M, S>>,
    net: &Network,
    tokens: Vec<Address>,
    spenders: Vec<Address>,
    min_allowance: U256,
) -> Result<()>
where
    M: Middleware + 'static,
    S: Signer + 'static,
{
    let me = sm.address();
    let dry = std::env::var("DRY_RUN").map(|v| v == "1").unwrap_or(false)
        || std::env::var("SAFE_LAUNCH")
            .map(|v| v == "1")
            .unwrap_or(false);
    let permit2 = if net.permit2.is_empty() {
        None
    } else {
        net.permit2.parse::<Address>().ok()
    };
    let permit2_max =
        U256::from_str_radix("ffffffffffffffffffffffffffffffffffffffff", 16).unwrap_or(U256::MAX);
    let permit2_exp: u64 = (1u64 << 48) - 1;

    for token in tokens {
        let c = IERC20::new(token, sm.clone());
        for spender in &spenders {
            match c.allowance(me, *spender).call().await {
                Ok(allowance) => {
                    if allowance < min_allowance {
                        let mut used_permit2 = false;
                        if let Some(p2addr) = permit2 {
                            if dry {
                                info!(
                                    "DRY: permit2 approve token={:?} spender={:?}",
                                    token, spender
                                );
                                used_permit2 = true;
                            } else {
                                let p2 = IPermit2::new(p2addr, sm.clone());
                                match p2
                                    .approve(token, *spender, permit2_max, permit2_exp, 0u64)
                                    .gas(80_000u64)
                                    .send()
                                    .await
                                {
                                    Ok(pending) => {
                                        let tx = pending.tx_hash();
                                        info!(
                                            "permit2 approve sent token={:?} spender={:?} tx={:?}",
                                            token, spender, tx
                                        );
                                        used_permit2 = true;
                                    }
                                    Err(e) => {
                                        info!(
                                            "permit2 approve failed token={:?} spender={:?} err={e:?}; falling back",
                                            token, spender
                                        );
                                    }
                                }
                            }
                        }
                        if !used_permit2 {
                            if dry {
                                info!("DRY: approve token={:?} spender={:?}", token, spender);
                            } else {
                                let call = c.approve(*spender, U256::MAX).gas(60_000u64);
                                let pending = call.send().await?;
                                let tx = pending.tx_hash();
                                info!(
                                    "approve sent token={:?} spender={:?} tx={:?}",
                                    token, spender, tx
                                );
                            }
                        }
                    } else {
                        debug!("allowance ok token={:?} spender={:?}", token, spender);
                    }
                }
                Err(e) => {
                    debug!(
                        "allowance check failed token={:?} spender={:?} err={e:?}",
                        token, spender
                    );
                }
            }
        }
    }
    Ok(())
}
