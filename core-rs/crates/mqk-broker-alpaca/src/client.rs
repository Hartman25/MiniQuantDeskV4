use crate::config::AlpacaConfig;
use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;
use serde::Serialize;

#[derive(Clone)]
pub struct AlpacaHttpClient {
    cfg: AlpacaConfig,
    http: reqwest::Client,
}

impl AlpacaHttpClient {
    pub fn new(cfg: AlpacaConfig) -> Self {
        let mut headers = HeaderMap::new();
        // Alpaca uses APCA-API-KEY-ID and APCA-API-SECRET-KEY headers.
        // NOTE: Do NOT log these values.
        headers.insert("APCA-API-KEY-ID", HeaderValue::from_str(&cfg.api_key).unwrap_or(HeaderValue::from_static("")));
        headers.insert("APCA-API-SECRET-KEY", HeaderValue::from_str(&cfg.api_secret).unwrap_or(HeaderValue::from_static("")));

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(cfg.timeout_secs))
            .build()
            .expect("reqwest client build");

        Self { cfg, http }
    }

    fn base(&self) -> String {
        self.cfg.base_url()
    }

    pub async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base(), path);
        let res = self.http.get(url).send().await?;
        let status = res.status();
        if !status.is_success() {
            anyhow::bail!("alpaca GET failed status={}", status);
        }
        Ok(res.json::<T>().await?)
    }

    pub async fn post_json<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let url = format!("{}{}", self.base(), path);
        let res = self.http.post(url).json(body).send().await?;
        let status = res.status();
        if !status.is_success() {
            anyhow::bail!("alpaca POST failed status={}", status);
        }
        Ok(res.json::<T>().await?)
    }

    pub async fn delete_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base(), path);
        let res = self.http.delete(url).send().await?;
        let status = res.status();
        if !status.is_success() {
            anyhow::bail!("alpaca DELETE failed status={}", status);
        }
        Ok(res.json::<T>().await?)
    }
}
