// src/job.rs
use crate::{JobContext, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a job
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(pub Uuid);

impl JobId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Job execution priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Low = 1,
    Normal = 5,
    High = 10,
    Critical = 20,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

/// Current state of a job
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Waiting,
    Active,
    Completed,
    Failed,
    Delayed,
    Paused,
}

/// Job execution options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOptions {
    pub delay_secs: Option<u64>,
    pub priority: Priority,
    pub attempts: u32,
    pub backoff: crate::backoff::BackoffStrategy,
    pub timeout_secs: Option<u64>,
    pub rate_limit_key: Option<String>,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self {
            delay_secs: None,
            priority: Priority::Normal,
            attempts: 3,
            backoff: crate::backoff::BackoffStrategy::Exponential { base: 2, cap: 300 },
            timeout_secs: Some(300), // 5 minutes default
            rate_limit_key: None,
        }
    }
}

/// Metadata for a job stored in Redis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobMetadata {
    pub id: JobId,
    pub name: String,
    pub queue_name: String,
    pub payload: serde_json::Value,
    pub options: JobOptions,
    pub state: JobState,
    pub attempts: u32,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub worker_id: Option<String>,
}

/// Core trait that all jobs must implement
#[async_trait::async_trait]
pub trait Job: Send + Sync + 'static + serde::de::DeserializeOwned + serde::Serialize {
    /// Execute the job with the provided context
    async fn perform(&self, ctx: &JobContext) -> Result<()>;

    /// Job type name for registration and deserialization
    fn name() -> &'static str
    where
        Self: Sized;

    /// Queue name for this job type
    fn queue_name() -> &'static str
    where
        Self: Sized,
    {
        "default"
    }

    /// Default options for this job type
    fn default_options() -> JobOptions
    where
        Self: Sized,
    {
        JobOptions::default()
    }
}
