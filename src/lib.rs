// src/lib.rs
//! ChainMQ: A Redis-backed job queue for Rust
//!
//! Inspired by BullMQ, this crate provides type-safe job queues with
//! Redis persistence, delayed execution, retries, and monitoring.

pub mod backoff;
pub mod context;
pub mod error;
pub mod job;
pub mod job_log_layer;
pub mod lua;
pub mod queue;
pub mod redis;
pub mod registry;
pub mod worker;

#[cfg(any(feature = "web-ui-axum", feature = "web-ui-actix"))]
pub mod web_ui;

pub use backoff::{Backoff, BackoffStrategy};
pub use context::{AppContext, JobContext};
pub use error::{ChainMQError, Result};
pub use job::JobMetadata;
pub use job::{Job, JobId, JobLogLine, JobOptions, JobState, Priority};
pub use job_log_layer::{JobLogLayer, job_logs_layer};
pub use queue::{Queue, QueueOptions};
pub use redis::RedisClient;
pub use registry::JobRegistry;
pub use worker::{Worker, WorkerBuilder};

#[cfg(any(feature = "web-ui-axum", feature = "web-ui-actix"))]
pub use web_ui::{WebUIAuth, WebUIMountConfig};

#[cfg(feature = "web-ui-axum")]
pub use web_ui::{WebUiState, chainmq_dashboard_router};

#[cfg(feature = "web-ui-actix")]
pub use web_ui::configure_chainmq_web_ui;

// Re-export commonly used types
pub use async_trait::async_trait;
pub use serde::{Deserialize, Serialize};
pub use serde_json;
