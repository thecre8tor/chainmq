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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_respects_cap() {
        let b = BackoffStrategy::Exponential { base: 2, cap: 8 };
        assert_eq!(b.calculate_delay(1).as_secs(), 2);
        assert_eq!(b.calculate_delay(3).as_secs(), 8);
        assert_eq!(b.calculate_delay(10).as_secs(), 8);
    }

    #[test]
    fn linear_increments() {
        let b = BackoffStrategy::Linear {
            increment: 5,
            cap: 100,
        };
        assert_eq!(b.calculate_delay(2).as_secs(), 10);
    }
}
