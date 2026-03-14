//! HTTP client for the rust-brain API

use crate::config::Config;
use crate::error::{McpError, Result};
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::time::Duration;
use tracing::{debug, instrument};

/// HTTP client wrapper for the rust-brain API
#[derive(Debug, Clone)]
pub struct ApiClient {
    client: Client,
    base_url: String,
}

impl ApiClient {
    /// Create a new API client
    pub fn new(config: &Config) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.http_timeout))
            .build()
            .map_err(McpError::Http)?;

        Ok(Self {
            client,
            base_url: config.api_base_url.clone(),
        })
    }

    /// Make a GET request to the API
    #[instrument(skip(self), fields(path = %path))]
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        debug!("GET {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(McpError::Http)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(McpError::Api(format!("{}: {}", status, body)));
        }

        response.json().await.map_err(McpError::Http)
    }

    /// Make a POST request to the API
    #[instrument(skip(self, body), fields(path = %path))]
    pub async fn post<T: DeserializeOwned, B: serde::Serialize + std::fmt::Debug>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), path);
        debug!("POST {} {:?}", url, body);

        let response = self
            .client
            .post(&url)
            .json(body)
            .send()
            .await
            .map_err(McpError::Http)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(McpError::Api(format!("{}: {}", status, body)));
        }

        response.json().await.map_err(McpError::Http)
    }

    /// Check if the API is healthy
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/health", self.base_url.trim_end_matches('/'));

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(McpError::Http)?;

        Ok(response.status().is_success())
    }
}
