//! rust-brain MCP Server
//!
//! A thin translation layer that exposes the rust-brain HTTP API as MCP tools.

pub mod config;
pub mod error;
pub mod client;
pub mod tools;

pub use config::Config;
pub use error::McpError;
pub use client::ApiClient;
