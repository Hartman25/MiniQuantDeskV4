use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlpacaEnv {
    Paper,
    Live,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlpacaConfig {
    pub env: AlpacaEnv,
    pub api_key: String,
    pub api_secret: String,
    pub base_url: Option<String>,
    pub timeout_secs: u64,
}

impl AlpacaConfig {
    pub fn base_url(&self) -> String {
        if let Some(u) = &self.base_url {
            return u.clone();
        }
        match self.env {
            AlpacaEnv::Paper => "https://paper-api.alpaca.markets".to_string(),
            AlpacaEnv::Live => "https://api.alpaca.markets".to_string(),
        }
    }
}
