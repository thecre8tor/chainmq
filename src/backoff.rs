// src/backoff.rs
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Backoff strategies for job retries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackoffStrategy {
    Fixed { seconds: u64 },
    Exponential { base: u64, cap: u64 },
    Linear { increment: u64, cap: u64 },
}

impl BackoffStrategy {
    pub fn calculate_delay(&self, attempt: u32) -> Duration {
        match self {
            BackoffStrategy::Fixed { seconds } => Duration::from_secs(*seconds),
            BackoffStrategy::Exponential { base, cap } => {
                let delay = base.saturating_pow(attempt).min(*cap);
                Duration::from_secs(delay)
            }
            BackoffStrategy::Linear { increment, cap } => {
                let delay = (increment * attempt as u64).min(*cap);
                Duration::from_secs(delay)
            }
        }
    }
}

impl Default for BackoffStrategy {
    fn default() -> Self {
        BackoffStrategy::Exponential { base: 2, cap: 300 }
    }
}

/// Convenience type alias
pub type Backoff = BackoffStrategy;
