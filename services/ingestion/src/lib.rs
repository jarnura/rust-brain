//! rust-brain Ingestion Library
//!
//! Re-exports the pipeline, parsing, embedding, graph, and monitoring modules
//! so they can be used by integration tests and external crates.

pub mod parsers;
pub mod typecheck;
pub mod pipeline;
pub mod graph;
pub mod embedding;
pub mod derive_detector;
pub mod monitoring;
