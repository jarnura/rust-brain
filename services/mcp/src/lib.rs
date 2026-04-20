//! rust-brain MCP Server
//!
//! A thin translation layer that exposes the rust-brain HTTP API as MCP tools.

pub mod client;
pub mod config;
pub mod error;
pub mod tools;

pub use client::ApiClient;
pub use config::Config;
pub use error::McpError;
