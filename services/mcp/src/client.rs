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

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    fn test_config(base_url: &str) -> Config {
        Config {
            transport: crate::config::Transport::Stdio,
            api_base_url: base_url.to_string(),
            http_timeout: 5,
            max_search_results: 50,
            default_search_limit: 10,
        }
    }

    #[test]
    fn test_client_new_success() {
        let config = test_config("http://localhost:8088");
        let client = ApiClient::new(&config);
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_client_get_success() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"value": 42}"#)
            .create_async()
            .await;

        let config = test_config(&server.url());
        let client = ApiClient::new(&config).unwrap();

        #[derive(serde::Deserialize)]
        struct TestResponse {
            value: i32,
        }

        let result: TestResponse = client.get("/test").await.unwrap();
        assert_eq!(result.value, 42);

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_get_error_status() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/notfound")
            .with_status(404)
            .with_body("Not found")
            .create_async()
            .await;

        let config = test_config(&server.url());
        let client = ApiClient::new(&config).unwrap();

        let result = client.get::<serde_json::Value>("/notfound").await;
        assert!(result.is_err());
        
        let err = result.unwrap_err();
        assert!(matches!(err, McpError::Api(_)));
        assert!(err.to_string().contains("404"));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_get_server_error() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/error")
            .with_status(500)
            .with_body("Internal server error")
            .create_async()
            .await;

        let config = test_config(&server.url());
        let client = ApiClient::new(&config).unwrap();

        let result = client.get::<serde_json::Value>("/error").await;
        assert!(result.is_err());
        
        let err = result.unwrap_err();
        assert!(matches!(err, McpError::Api(_)));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_post_success() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/create")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id": 1, "name": "test"}"#)
            .match_body(mockito::Matcher::JsonString(serde_json::json!({"name": "test"}).to_string()))
            .create_async()
            .await;

        let config = test_config(&server.url());
        let client = ApiClient::new(&config).unwrap();

        #[derive(Debug, serde::Deserialize)]
        struct CreateResponse {
            id: i32,
            name: String,
        }

        #[derive(Debug, serde::Serialize)]
        struct CreateRequest {
            name: String,
        }

        let result: CreateResponse = client
            .post("/create", &CreateRequest { name: "test".to_string() })
            .await
            .unwrap();
        
        assert_eq!(result.id, 1);
        assert_eq!(result.name, "test");

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_post_error_status() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/create")
            .with_status(400)
            .with_body("Bad request")
            .create_async()
            .await;

        let config = test_config(&server.url());
        let client = ApiClient::new(&config).unwrap();

        let result = client
            .post::<serde_json::Value, serde_json::Value>("/create", &serde_json::json!({}))
            .await;
        
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, McpError::Api(_)));

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_health_check_success() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/health")
            .with_status(200)
            .create_async()
            .await;

        let config = test_config(&server.url());
        let client = ApiClient::new(&config).unwrap();

        let healthy = client.health_check().await.unwrap();
        assert!(healthy);

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_health_check_failure() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/health")
            .with_status(503)
            .create_async()
            .await;

        let config = test_config(&server.url());
        let client = ApiClient::new(&config).unwrap();

        let healthy = client.health_check().await.unwrap();
        assert!(!healthy);

        mock.assert_async().await;
    }

    #[test]
    fn test_client_clone() {
        let config = test_config("http://localhost:8088");
        let client = ApiClient::new(&config).unwrap();
        let cloned = client.clone();
        
        // Both should work
        assert_eq!(client.base_url, cloned.base_url);
    }

    #[tokio::test]
    async fn test_client_invalid_json_response() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/invalid")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("not valid json")
            .create_async()
            .await;

        let config = test_config(&server.url());
        let client = ApiClient::new(&config).unwrap();

        let result = client.get::<serde_json::Value>("/invalid").await;
        assert!(result.is_err());

        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_client_connection_error() {
        // Use an invalid URL that won't connect
        let config = test_config("http://127.0.0.1:1");
        let client = ApiClient::new(&config).unwrap();

        let result = client.get::<serde_json::Value>("/test").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), McpError::Http(_)));
    }
}
