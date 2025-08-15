use crate::config::{Config, Network};
use anyhow::{anyhow, Result};
use ethers::providers::{Http, Provider, ProviderError};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tracing::warn;

#[derive(Clone)]
pub struct ChainClient {
    pub cfg: Network,
    endpoints: Vec<String>,
    inner: Arc<Mutex<ClientState>>,
}

struct ClientState {
    current_index: usize,
    provider: Arc<Provider<Http>>,
}

impl ChainClient {
    pub fn provider(&self) -> Arc<Provider<Http>> {
        self.inner.lock().unwrap().provider.clone()
    }

    fn build_provider(url: &str) -> Result<Provider<Http>> {
        let req_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(12))
            .build()?;
        let url: reqwest::Url = reqwest::Url::parse(url)?;
        let http = Http::new_with_client(url, req_client);
        Ok(Provider::new(http).interval(Duration::from_millis(500)))
    }

    fn switch_provider(&self) -> Result<()> {
        let (next_idx, url) = {
            let st = self.inner.lock().unwrap();
            let next = (st.current_index + 1) % self.endpoints.len();
            (next, self.endpoints[next].clone())
        };

        let provider = Arc::new(Self::build_provider(&url)?);
        {
            let mut st = self.inner.lock().unwrap();
            st.current_index = next_idx;
            st.provider = provider;
        }
        warn!("RPC failover to {url}");
        Ok(())
    }

    fn is_retryable(err: &anyhow::Error) -> bool {
        if let Some(pe) = err.downcast_ref::<ProviderError>() {
            if let ProviderError::JsonRpcClientError(_) = pe {
                return true;
            }
        }
        if let Some(req_err) = err.downcast_ref::<reqwest::Error>() {
            return req_err.is_timeout() || req_err.is_connect();
        }
        false
    }

    pub async fn with_failover<T, Fut, E>(&self, op: impl Fn(Arc<Provider<Http>>) -> Fut) -> Result<T>
    where
        Fut: Future<Output = Result<T, E>>,
        E: Into<anyhow::Error> + Send + Sync + 'static,
    {
        let mut last_err: Option<anyhow::Error> = None;
        for _ in 0..self.endpoints.len() {
            let provider = self.provider();
            match op(provider.clone()).await.map_err(|e| e.into()) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if !Self::is_retryable(&e) {
                        return Err(e);
                    }
                    last_err = Some(e);
                    self.switch_provider()?;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("all RPC endpoints failed")))
    }
}

#[derive(Clone)]
pub struct MultiChain {
    pub clients: HashMap<u64, ChainClient>,
}

impl MultiChain {
    pub async fn from_config(cfg: &Config) -> Result<Self> {
        let mut map = HashMap::new();

        for n in &cfg.networks {
            if n.rpc.is_empty() {
                return Err(anyhow!("network '{}' has no RPC endpoints", n.name));
            }
            let provider = Arc::new(ChainClient::build_provider(&n.rpc[0])?);

            if map.contains_key(&n.chain_id) {
                return Err(anyhow!("duplicate chain_id in config: {}", n.chain_id));
            }

            let inner = ClientState {
                current_index: 0,
                provider,
            };

            map.insert(
                n.chain_id,
                ChainClient {
                    cfg: n.clone(),
                    endpoints: n.rpc.clone(),
                    inner: Arc::new(Mutex::new(inner)),
                },
            );
        }

        Ok(Self { clients: map })
    }
}