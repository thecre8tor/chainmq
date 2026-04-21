// src/lib.rs
//! ChainMQ: A Redis-backed job queue for Rust
//!
//! Inspired by BullMQ, this crate provides type-safe job queues with
//! Redis persistence, delayed execution, retries, and monitoring.

pub mod backoff;
pub mod context;
pub mod error;
pub mod job;
pub mod lua;
pub mod queue;
pub mod registry;
pub mod worker;

#[cfg(feature = "web-ui")]
pub mod web_ui;

pub use backoff::{Backoff, BackoffStrategy};
pub use context::{AppContext, JobContext};
pub use error::{ChainMQError, Result};
pub use job::JobMetadata;
pub use job::{Job, JobId, JobOptions, JobState, Priority};
pub use queue::{Queue, QueueOptions};
pub use registry::JobRegistry;
pub use worker::{Worker, WorkerBuilder};

#[cfg(feature = "web-ui")]
pub use web_ui::{start_web_ui, start_web_ui_simple, WebUIConfig};

// Provide a no-op version when web-ui feature is disabled
#[cfg(not(feature = "web-ui"))]
pub async fn start_web_ui_simple(_queue: Queue) -> std::io::Result<()> {
    Ok(())
}

// Re-export commonly used types
pub use async_trait::async_trait;
pub use serde::{Deserialize, Serialize};
pub use serde_json;
