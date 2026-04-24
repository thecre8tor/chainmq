//! Repeatable job schedule metadata (Redis-backed; see `docs/repeatables-cron-design.md`).

use serde::{Deserialize, Serialize};

/// How to behave when multiple ticks were missed (interval schedules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RepeatCatchUp {
    /// Advance to a single next fire (default).
    #[default]
    One,
    All,
    None,
}

/// One row returned by [`crate::Queue::list_repeats`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepeatScheduleInfo {
    pub schedule_id: String,
    pub next_run_unix: i64,
    pub queue_name: String,
    pub job_name: String,
    pub enabled: bool,
    pub interval_secs: Option<u64>,
    #[serde(default)]
    pub cron_expr: Option<String>,
    #[serde(default)]
    pub catch_up: RepeatCatchUp,
}
