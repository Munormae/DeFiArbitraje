use std::sync::Arc;

use anyhow::Result;
use ethers::prelude::*;
use tracing::{debug, info};

use crate::config::Network;

abigen!(
    IERC20,
    r#"[function allowance(address owner,address spender)view returns(uint256)
                     function approve(address spender,uint256 amount) returns(bool)]"#
);

pub async fn ensure_approvals<M, S>(
    sm: Arc<SignerMiddleware<M, S>>,
    _net: &Network,
    tokens: Vec<Address>,
    spenders: Vec<Address>,
    min_allowance: U256,
) -> Result<()>
where
    M: Middleware + 'static,
    S: Signer + 'static,
{
    let me = sm.address();
    let dry = std::env::var("DRY_RUN").is_ok() || std::env::var("SAFE_LAUNCH").is_ok();

    for token in tokens {
        let c = IERC20::new(token, sm.clone());
        for spender in &spenders {
            match c.allowance(me, *spender).call().await {
                Ok(allowance) => {
                    if allowance < min_allowance {
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
