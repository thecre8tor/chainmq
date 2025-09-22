// src/lib.rs
//! rustque: A Redis-backed job queue for Rust
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

pub use backoff::{Backoff, BackoffStrategy};
pub use context::{AppContext, JobContext};
pub use error::{Result, RustqueError};
pub use job::JobMetadata;
pub use job::{Job, JobId, JobOptions, JobState, Priority};
pub use queue::{Queue, QueueOptions};
pub use registry::JobRegistry;
pub use worker::{Worker, WorkerBuilder};

// Re-export commonly used types
pub use async_trait::async_trait;
pub use serde::{Deserialize, Serialize};
