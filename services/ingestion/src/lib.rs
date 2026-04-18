//! rust-brain Ingestion Library
//!
//! Re-exports the pipeline, parsing, embedding, graph, and monitoring modules
//! so they can be used by integration tests and external crates.

pub mod derive_detector;
pub mod embedding;
pub mod graph;
pub mod monitoring;
pub mod parsers;
pub mod pipeline;
pub mod typecheck;
