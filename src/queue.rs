// src/queue.rs
use crate::{Job, JobId, JobMetadata, JobOptions, JobState, Result, RsBullError, lua::LuaScripts};
use chrono::Utc;
use redis::{Client as RedisClient, Commands};
use serde_json;
use std::time::{SystemTime, UNIX_EPOCH};

/// Options for queue configuration
#[derive(Debug, Clone)]
pub struct QueueOptions {
    pub name: String,
    pub redis_url: String,
    pub key_prefix: String,
    pub default_concurrency: usize,
    pub max_stalled_interval: u64,
}

impl Default for QueueOptions {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            redis_url: "redis://127.0.0.1:6379".to_string(),
            key_prefix: "rbq".to_string(),
            default_concurrency: 10,
            max_stalled_interval: 30000, // 30 seconds
        }
    }
}

/// Redis-backed job queue
pub struct Queue {
    client: RedisClient,
    options: QueueOptions,
    scripts: LuaScripts,
}

impl Queue {
    pub async fn new(options: QueueOptions) -> Result<Self> {
        let client = RedisClient::open(options.redis_url.as_str())?;
        let scripts = LuaScripts::new(&client).await?;

        Ok(Self {
            client,
            options,
            scripts,
        })
    }

    /// Enqueue a job with default options
    pub async fn enqueue<T: Job>(&self, job: T) -> Result<JobId> {
        self.enqueue_with_options(job, T::default_options()).await
    }

    /// Enqueue a job with custom options
    pub async fn enqueue_with_options<T: Job>(&self, job: T, options: JobOptions) -> Result<JobId> {
        let job_id = JobId::new();
        let now = Utc::now();

        let metadata = JobMetadata {
            id: job_id.clone(),
            name: T::name().to_string(),
            queue_name: T::queue_name().to_string(),
            payload: serde_json::to_value(&job)?,
            options: options.clone(),
            state: if options.delay_secs.is_some() {
                JobState::Delayed
            } else {
                JobState::Waiting
            },
            attempts: 0,
            created_at: now,
            started_at: None,
            completed_at: None,
            failed_at: None,
            last_error: None,
            worker_id: None,
        };

        let mut con = self.client.get_connection()?;

        // Store job metadata
        let job_key = self.job_key(&job_id);
        let metadata_json = serde_json::to_string(&metadata)?;
        let _: () = con.hset(&job_key, "metadata", metadata_json)?;

        // Add to appropriate queue
        if let Some(delay_secs) = options.delay_secs {
            let execute_at = now.timestamp() + delay_secs as i64;
            let delayed_key = self.delayed_key(T::queue_name());
            let _: () = con.zadd(&delayed_key, job_id.to_string(), execute_at)?;
        } else {
            let wait_key = self.wait_key(T::queue_name());
            let _: () = con.lpush(&wait_key, job_id.to_string())?;
        }

        // Metrics removed

        Ok(job_id)
    }

    /// Get job by ID
    pub async fn get_job(&self, job_id: &JobId) -> Result<Option<JobMetadata>> {
        let mut con = self.client.get_connection()?;
        let job_key = self.job_key(job_id);

        let metadata_json: Option<String> = con.hget(&job_key, "metadata")?;

        match metadata_json {
            Some(json) => {
                let metadata: JobMetadata = serde_json::from_str(&json)?;
                Ok(Some(metadata))
            }
            None => Ok(None),
        }
    }

    /// Move delayed jobs to waiting queue
    pub async fn process_delayed(&self, queue_name: &str) -> Result<usize> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // move_delayed.lua expects ARGV[1] = queue_name, ARGV[2] = current_timestamp
        let result: i64 = self
            .scripts
            .move_delayed
            .arg(queue_name)
            .arg(now)
            .invoke_async(&mut self.client.get_async_connection().await?)
            .await?;

        Ok(result as usize)
    }

    /// Claim next job for processing
    pub async fn claim_job(&self, queue_name: &str, worker_id: &str) -> Result<Option<JobId>> {
        let now = Utc::now().timestamp();
        let wait_key = self.wait_key(queue_name);
        let active_key = self.active_key(queue_name);

        let result: Option<String> = self
            .scripts
            .claim_job
            .key(&wait_key)
            .key(&active_key)
            .arg(worker_id)
            .arg(now)
            .invoke_async(&mut self.client.get_async_connection().await?)
            .await?;

        match result {
            Some(job_id_str) => {
                let job_id = JobId(
                    job_id_str
                        .parse()
                        .map_err(|_| RsBullError::Worker("Invalid job ID format".to_string()))?,
                );
                Ok(Some(job_id))
            }
            None => Ok(None),
        }
    }

    /// Complete a job successfully
    pub async fn complete_job(&self, job_id: &JobId, queue_name: &str) -> Result<()> {
        let mut con = self.client.get_connection()?;
        let job_key = self.job_key(job_id);
        let active_key = self.active_key(queue_name);

        // Update job metadata
        let now = Utc::now();
        let _: () = con.hset_multiple(
            &job_key,
            &[
                ("state", serde_json::to_string(&JobState::Completed)?),
                ("completed_at", now.to_rfc3339()),
            ],
        )?;

        // Remove from active set
        let _: () = con.srem(&active_key, job_id.to_string())?;

        // Metrics removed
        Ok(())
    }

    /// Fail a job and potentially retry or move to failed queue
    pub async fn fail_job(
        &self,
        job_id: &JobId,
        queue_name: &str,
        error: &str,
        metadata: &JobMetadata,
    ) -> Result<()> {
        let mut con = self.client.get_connection()?;
        let job_key = self.job_key(job_id);
        let active_key = self.active_key(queue_name);

        let new_attempts = metadata.attempts + 1;

        if new_attempts < metadata.options.attempts {
            // Retry with backoff
            let delay = metadata.options.backoff.calculate_delay(new_attempts);
            let execute_at = Utc::now().timestamp() + delay.as_secs() as i64;

            // Update metadata
            let _: () = con.hset_multiple(
                &job_key,
                &[
                    ("state", serde_json::to_string(&JobState::Delayed)?),
                    ("attempts", new_attempts.to_string()),
                    ("last_error", error.to_string()),
                    ("failed_at", Utc::now().to_rfc3339()),
                ],
            )?;

            // Schedule retry
            let delayed_key = self.delayed_key(queue_name);
            let _: () = con.zadd(&delayed_key, job_id.to_string(), execute_at)?;
        } else {
            // Final failure - move to failed queue
            let _: () = con.hset_multiple(
                &job_key,
                &[
                    ("state", serde_json::to_string(&JobState::Failed)?),
                    ("attempts", new_attempts.to_string()),
                    ("last_error", error.to_string()),
                    ("failed_at", Utc::now().to_rfc3339()),
                ],
            )?;

            let failed_key = self.failed_key(queue_name);
            let _: () = con.lpush(&failed_key, job_id.to_string())?;

            // Metrics removed
        }

        // Remove from active set
        let _: () = con.srem(&active_key, job_id.to_string())?;

        Ok(())
    }

    /// Get queue statistics
    pub async fn get_stats(&self, queue_name: &str) -> Result<QueueStats> {
        let mut con = self.client.get_connection()?;

        let wait_len: usize = con.llen(self.wait_key(queue_name))?;
        let active_len: usize = con.scard(self.active_key(queue_name))?;
        let delayed_len: usize = con.zcard(self.delayed_key(queue_name))?;
        let failed_len: usize = con.llen(self.failed_key(queue_name))?;

        Ok(QueueStats {
            waiting: wait_len,
            active: active_len,
            delayed: delayed_len,
            failed: failed_len,
        })
    }

    // Redis key helpers
    fn job_key(&self, job_id: &JobId) -> String {
        format!("{}:job:{}", self.options.key_prefix, job_id)
    }

    fn wait_key(&self, queue_name: &str) -> String {
        format!("{}:queue:{}:wait", self.options.key_prefix, queue_name)
    }

    fn active_key(&self, queue_name: &str) -> String {
        format!("{}:queue:{}:active", self.options.key_prefix, queue_name)
    }

    fn delayed_key(&self, queue_name: &str) -> String {
        format!("{}:queue:{}:delayed", self.options.key_prefix, queue_name)
    }

    fn failed_key(&self, queue_name: &str) -> String {
        format!("{}:queue:{}:failed", self.options.key_prefix, queue_name)
    }
}

#[derive(Debug, Clone)]
pub struct QueueStats {
    pub waiting: usize,
    pub active: usize,
    pub delayed: usize,
    pub failed: usize,
}
