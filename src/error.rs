// src/error.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RsBullError {
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Job not found: {0}")]
    JobNotFound(crate::JobId),

    #[error("Queue not found: {0}")]
    QueueNotFound(String),

    #[error("Job execution failed: {0}")]
    JobExecution(#[from] anyhow::Error),

    #[error("Worker error: {0}")]
    Worker(String),

    #[error("Registry error: {0}")]
    Registry(String),

    #[error("Rate limited")]
    RateLimited,

    #[error("Timeout")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, RsBullError>;
