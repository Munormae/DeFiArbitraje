use crate::config::{Config, Network};
use anyhow::{anyhow, Result};
use ethers::providers::{Http, Provider};
use std::{collections::HashMap, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct ChainClient {
    pub cfg: Network,
    pub provider: Arc<Provider<Http>>,
}

#[derive(Clone)]
pub struct MultiChain {
    pub clients: HashMap<u64, ChainClient>,
}

impl MultiChain {
    pub async fn from_config(cfg: &Config) -> Result<Self> {
        let mut map = HashMap::new();

        for n in &cfg.networks {
            let rpc = n
                .rpc
                .get(0)
                .ok_or_else(|| anyhow!("network '{}' has no RPC endpoints", n.name))?
                .clone();

            // reqwest-клиент с таймаутом
            let req_client = reqwest::Client::builder()
                .timeout(Duration::from_secs(12))
                .build()?;

            // Нужен именно reqwest::Url
            let url: reqwest::Url = reqwest::Url::parse(&rpc)?;

            // ПРАВИЛЬНЫЙ порядок: (url, client)
            let http = Http::new_with_client(url, req_client);
            let provider = Provider::new(http).interval(Duration::from_millis(500));

            if map.contains_key(&n.chain_id) {
                return Err(anyhow!("duplicate chain_id in config: {}", n.chain_id));
            }

            map.insert(
                n.chain_id,
                ChainClient {
                    cfg: n.clone(),
                    provider: Arc::new(provider),
                },
            );
        }

        Ok(Self { clients: map })
    }
}