use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub version: String,
    pub created_at: String,
    pub networks: Vec<Network>,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let s = std::fs::read_to_string(path)?;
        let c: Self = serde_json::from_str(&s)?;
        Ok(c)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub chainId: u64,
    pub rpc: Vec<String>,
    pub tokens: HashMap<String, Token>,
    pub dexes: Vec<DexConfig>,
    pub pairs: Vec<[String; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Token {
    pub address: String,
    pub decimals: u8,
    #[serde(default)]
    pub optional: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DexConfig {
    pub name: String,
    #[serde(rename="type")]
    pub dex_type: String, // "v2" | "v3" | "solidly_v2"
    pub factory: Option<String>,
    pub router: Option<String>,
    pub feeTiers_bps: Option<Vec<u32>>,
    pub stablePools: Option<bool>,
}
