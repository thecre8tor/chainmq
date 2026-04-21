// src/queue.rs
use crate::{
    ChainMQError, Job, JobId, JobLogLine, JobMetadata, JobOptions, JobState, Result,
    lua::LuaScripts,
};
use chrono::Utc;
use redis::aio::MultiplexedConnection;
use redis::{AsyncCommands, Client as RedisClient};
use serde_json;
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

/// Options for queue configuration
#[derive(Debug, Clone)]
pub struct QueueOptions {
    pub name: String,
    pub redis_url: String,
    pub redis_instance: Option<RedisClient>,
    pub key_prefix: String,
    pub default_concurrency: usize,
    pub max_stalled_interval: u64,
    /// Max log lines retained per job in Redis (older lines dropped). Used by job log tracing layer.
    pub job_logs_max_lines: usize,
}

impl Default for QueueOptions {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            redis_url: "redis://127.0.0.1:6379".to_string(),
            redis_instance: None,
            key_prefix: "rbq".to_string(),
            default_concurrency: 10,
            max_stalled_interval: 30000, // 30 seconds
            job_logs_max_lines: 500,
        }
    }
}

/// Redis-backed job queue
pub struct Queue {
    options: QueueOptions,
    scripts: LuaScripts,
    async_conn: MultiplexedConnection,
}

impl Queue {
    pub async fn new(options: QueueOptions) -> Result<Self> {
        let client = match options.clone().redis_instance {
            Some(client) => client,
            None => RedisClient::open(options.redis_url.as_str())?,
        };

        let scripts = LuaScripts::new(&client).await?;

        let async_conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(ChainMQError::Redis)?;

        Ok(Self {
            options,
            scripts,
            async_conn,
        })
    }

    fn queues_registry_key(&self) -> String {
        format!("{}:queues", self.options.key_prefix)
    }

    async fn save_job_metadata(&self, job_id: &JobId, metadata: &JobMetadata) -> Result<()> {
        let mut conn = self.async_conn.clone();
        let job_key = self.job_key(job_id);
        let json = serde_json::to_string(metadata)?;
        let _: () = conn
            .hset::<_, _, _, ()>(&job_key, "metadata", json)
            .await
            .map_err(ChainMQError::Redis)?;
        Ok(())
    }

    async fn apply_claimed_metadata(&self, job_id: &JobId, worker_id: &str) -> Result<()> {
        let mut metadata = match self.get_job(job_id).await? {
            Some(m) => m,
            None => {
                return Err(ChainMQError::Worker(format!(
                    "Claimed job {} has no metadata",
                    job_id
                )));
            }
        };
        let now = Utc::now();
        metadata.state = JobState::Active;
        metadata.started_at = Some(now);
        metadata.worker_id = Some(worker_id.to_string());
        self.save_job_metadata(job_id, &metadata).await
    }

    /// Return a claimed job to the waiting queue (e.g. worker shutdown before execution).
    pub async fn requeue_claimed_job(&self, job_id: &JobId, queue_name: &str) -> Result<()> {
        let mut conn = self.async_conn.clone();
        let active_key = self.active_key(queue_name);
        let wait_key = self.wait_key(queue_name);

        let _: i32 = conn
            .srem::<_, _, i32>(&active_key, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .lpush::<_, _, ()>(&wait_key, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;

        if let Some(mut metadata) = self.get_job(job_id).await? {
            metadata.state = JobState::Waiting;
            metadata.started_at = None;
            metadata.worker_id = None;
            self.save_job_metadata(job_id, &metadata).await?;
        }

        Ok(())
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
            response: None,
        };

        let mut conn = self.async_conn.clone();
        let job_key = self.job_key(&job_id);
        let metadata_json = serde_json::to_string(&metadata)?;
        let _: () = conn
            .hset::<_, _, _, ()>(&job_key, "metadata", metadata_json)
            .await
            .map_err(ChainMQError::Redis)?;

        let qname = T::queue_name();
        let registry_key = self.queues_registry_key();
        let _: () = conn
            .sadd::<_, _, ()>(&registry_key, qname)
            .await
            .map_err(ChainMQError::Redis)?;

        if let Some(delay_secs) = options.delay_secs {
            let execute_at = now.timestamp() + delay_secs as i64;
            let delayed_key = self.delayed_key(qname);
            let _: () = conn
                .zadd::<_, _, _, ()>(&delayed_key, job_id.to_string(), execute_at)
                .await
                .map_err(ChainMQError::Redis)?;
        } else {
            let wait_key = self.wait_key(qname);
            let _: () = conn
                .lpush::<_, _, ()>(&wait_key, job_id.to_string())
                .await
                .map_err(ChainMQError::Redis)?;
        }

        Ok(job_id)
    }

    /// Get job by ID (single `metadata` JSON field — source of truth)
    pub async fn get_job(&self, job_id: &JobId) -> Result<Option<JobMetadata>> {
        let mut conn = self.async_conn.clone();
        let job_key = self.job_key(job_id);

        let metadata_json: Option<String> = conn
            .hget(&job_key, "metadata")
            .await
            .map_err(ChainMQError::Redis)?;

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

        let result_str: String = self
            .scripts
            .move_delayed
            .arg(queue_name)
            .arg(now)
            .arg(&self.options.key_prefix)
            .invoke_async(&mut self.async_conn.clone())
            .await?;

        if result_str.is_empty() {
            return Ok(0);
        }

        let ids: Vec<&str> = result_str.split(',').filter(|s| !s.is_empty()).collect();
        for id_str in &ids {
            if let Ok(uuid) = id_str.parse::<uuid::Uuid>() {
                let job_id = JobId(uuid);
                if let Some(mut metadata) = self.get_job(&job_id).await?
                    && metadata.state != JobState::Waiting
                {
                    metadata.state = JobState::Waiting;
                    self.save_job_metadata(&job_id, &metadata).await?;
                }
            }
        }

        Ok(ids.len())
    }

    /// Claim next job for processing
    pub async fn claim_job(&self, queue_name: &str, worker_id: &str) -> Result<Option<JobId>> {
        let wait_key = self.wait_key(queue_name);
        let active_key = self.active_key(queue_name);

        let result: Option<String> = self
            .scripts
            .claim_job
            .key(&wait_key)
            .key(&active_key)
            .invoke_async(&mut self.async_conn.clone())
            .await?;

        match result {
            Some(job_id_str) => {
                let job_id = JobId(
                    job_id_str
                        .parse()
                        .map_err(|_| ChainMQError::Worker("Invalid job ID format".to_string()))?,
                );
                self.apply_claimed_metadata(&job_id, worker_id).await?;
                Ok(Some(job_id))
            }
            None => Ok(None),
        }
    }

    /// Complete a job successfully. `response` is stored on the job metadata (e.g. from
    /// [`crate::JobContext::set_response`]) and visible in the dashboard API.
    pub async fn complete_job(
        &self,
        job_id: &JobId,
        queue_name: &str,
        response: Option<serde_json::Value>,
    ) -> Result<()> {
        let mut metadata = match self.get_job(job_id).await? {
            Some(m) => m,
            None => return Err(ChainMQError::Worker("Job not found".to_string())),
        };

        let mut conn = self.async_conn.clone();
        let job_key = self.job_key(job_id);
        let active_key = self.active_key(queue_name);
        let completed_key = self.completed_key(queue_name);

        let now = Utc::now();
        metadata.state = JobState::Completed;
        metadata.completed_at = Some(now);
        metadata.response = response;

        let metadata_json = serde_json::to_string(&metadata)?;
        let _: () = conn
            .hset::<_, _, _, ()>(&job_key, "metadata", metadata_json)
            .await
            .map_err(ChainMQError::Redis)?;

        let _: () = conn
            .srem::<_, _, ()>(&active_key, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;

        let _: () = conn
            .lpush::<_, _, ()>(&completed_key, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;

        let _: () = conn
            .ltrim(&completed_key, 0, 999)
            .await
            .map_err(ChainMQError::Redis)?;

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
        let mut conn = self.async_conn.clone();
        let active_key = self.active_key(queue_name);

        let mut meta = metadata.clone();
        let new_attempts = meta.attempts + 1;
        meta.attempts = new_attempts;
        meta.last_error = Some(error.to_string());
        meta.failed_at = Some(Utc::now());

        if new_attempts < meta.options.attempts {
            let delay = meta.options.backoff.calculate_delay(new_attempts);
            let execute_at = Utc::now().timestamp() + delay.as_secs() as i64;
            meta.state = JobState::Delayed;

            self.save_job_metadata(job_id, &meta).await?;

            let delayed_key = self.delayed_key(queue_name);
            let _: () = conn
                .zadd::<_, _, _, ()>(&delayed_key, job_id.to_string(), execute_at)
                .await
                .map_err(ChainMQError::Redis)?;
        } else {
            meta.state = JobState::Failed;
            self.save_job_metadata(job_id, &meta).await?;

            let failed_key = self.failed_key(queue_name);
            let _: () = conn
                .lpush::<_, _, ()>(&failed_key, job_id.to_string())
                .await
                .map_err(ChainMQError::Redis)?;
        }

        let _: () = conn
            .srem::<_, _, ()>(&active_key, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;

        Ok(())
    }

    /// Get queue statistics
    pub async fn get_stats(&self, queue_name: &str) -> Result<QueueStats> {
        let mut conn = self.async_conn.clone();

        let wait_len: usize = conn
            .llen(self.wait_key(queue_name))
            .await
            .map_err(ChainMQError::Redis)?;
        let active_len: usize = conn
            .scard(self.active_key(queue_name))
            .await
            .map_err(ChainMQError::Redis)?;
        let delayed_len: usize = conn
            .zcard(self.delayed_key(queue_name))
            .await
            .map_err(ChainMQError::Redis)?;
        let failed_len: usize = conn
            .llen(self.failed_key(queue_name))
            .await
            .map_err(ChainMQError::Redis)?;
        let completed_len: usize = conn
            .llen(self.completed_key(queue_name))
            .await
            .map_err(ChainMQError::Redis)?;

        Ok(QueueStats {
            waiting: wait_len,
            active: active_len,
            delayed: delayed_len,
            failed: failed_len,
            completed: completed_len,
        })
    }

    async fn scan_match(&self, pattern: &str) -> Result<Vec<String>> {
        let mut conn = self.async_conn.clone();
        let mut cursor = 0u64;
        let mut all = Vec::new();
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(256)
                .query_async(&mut conn)
                .await
                .map_err(ChainMQError::Redis)?;
            all.extend(keys);
            if next == 0 {
                break;
            }
            cursor = next;
        }
        Ok(all)
    }

    /// List jobs by state
    pub async fn list_jobs(
        &self,
        queue_name: &str,
        state: JobState,
        limit: Option<usize>,
    ) -> Result<Vec<JobMetadata>> {
        let mut conn = self.async_conn.clone();
        let limit = limit.unwrap_or(100);
        let job_ids: Vec<String> = match state {
            JobState::Waiting => {
                let wait_key = self.wait_key(queue_name);
                conn.lrange(&wait_key, 0, (limit - 1) as isize)
                    .await
                    .map_err(ChainMQError::Redis)?
            }
            JobState::Active => {
                let active_key = self.active_key(queue_name);
                let ids: Vec<String> = conn
                    .smembers(&active_key)
                    .await
                    .map_err(ChainMQError::Redis)?;
                ids.into_iter().take(limit).collect()
            }
            JobState::Delayed => {
                let delayed_key = self.delayed_key(queue_name);
                let ids: Vec<String> = redis::cmd("ZRANGEBYSCORE")
                    .arg(&delayed_key)
                    .arg("-inf")
                    .arg("+inf")
                    .arg("LIMIT")
                    .arg(0)
                    .arg(limit)
                    .query_async(&mut conn)
                    .await
                    .map_err(ChainMQError::Redis)?;
                ids
            }
            JobState::Failed => {
                let failed_key = self.failed_key(queue_name);
                conn.lrange(&failed_key, 0, (limit - 1) as isize)
                    .await
                    .map_err(ChainMQError::Redis)?
            }
            JobState::Completed => {
                let completed_key = self.completed_key(queue_name);
                conn.lrange(&completed_key, 0, (limit - 1) as isize)
                    .await
                    .map_err(ChainMQError::Redis)?
            }
            _ => {
                return Ok(Vec::new());
            }
        };

        let mut jobs = Vec::new();

        for job_id_str in job_ids {
            if let Ok(job_id) = job_id_str.parse::<uuid::Uuid>() {
                let job_id = JobId(job_id);

                let is_in_expected_list = match state {
                    JobState::Waiting => {
                        let wait_key = self.wait_key(queue_name);
                        let list: Vec<String> = self
                            .async_conn
                            .clone()
                            .lrange(&wait_key, 0, -1)
                            .await
                            .map_err(ChainMQError::Redis)?;
                        list.contains(&job_id_str)
                    }
                    JobState::Active => {
                        let active_key = self.active_key(queue_name);
                        self.async_conn
                            .clone()
                            .sismember::<_, _, bool>(&active_key, &job_id_str)
                            .await
                            .map_err(ChainMQError::Redis)?
                    }
                    JobState::Delayed => {
                        let delayed_key = self.delayed_key(queue_name);
                        match self
                            .async_conn
                            .clone()
                            .zrank::<&str, &str, Option<i64>>(&delayed_key, &job_id_str)
                            .await
                        {
                            Ok(Some(_)) => true,
                            Ok(None) => false,
                            Err(_) => false,
                        }
                    }
                    JobState::Failed => {
                        let failed_key = self.failed_key(queue_name);
                        let list: Vec<String> = self
                            .async_conn
                            .clone()
                            .lrange(&failed_key, 0, -1)
                            .await
                            .map_err(ChainMQError::Redis)?;
                        list.contains(&job_id_str)
                    }
                    JobState::Completed => {
                        let completed_key = self.completed_key(queue_name);
                        let list: Vec<String> = self
                            .async_conn
                            .clone()
                            .lrange(&completed_key, 0, -1)
                            .await
                            .map_err(ChainMQError::Redis)?;
                        list.contains(&job_id_str)
                    }
                    _ => false,
                };

                if !is_in_expected_list {
                    tracing::warn!(
                        "Job {} was listed for {:?} state but is not actually in that list - skipping",
                        job_id_str,
                        state
                    );
                    continue;
                }

                match self.get_job(&job_id).await {
                    Ok(Some(mut metadata)) => {
                        let expected_state = state.clone();
                        if metadata.state != expected_state {
                            metadata.state = expected_state.clone();
                            self.save_job_metadata(&job_id, &metadata).await?;
                        }

                        jobs.push(metadata);
                    }
                    Ok(None) => {
                        tracing::warn!(
                            "Job {} found in {:?} list but metadata not found",
                            job_id_str,
                            state
                        );
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to get job {}: {}", job_id_str, e);
                        continue;
                    }
                }
            } else {
                tracing::warn!("Invalid job ID format: {}", job_id_str);
            }
        }

        Ok(jobs)
    }

    /// Discover queue names via registry set and SCAN (legacy keys without registry)
    pub async fn list_queues(&self) -> Result<Vec<String>> {
        let mut conn = self.async_conn.clone();
        let mut queues: HashSet<String> = HashSet::new();

        let registered: Vec<String> = conn
            .smembers(self.queues_registry_key())
            .await
            .map_err(ChainMQError::Redis)?;
        for q in registered {
            queues.insert(q);
        }

        for suffix in &[":wait", ":active", ":delayed", ":failed", ":completed"] {
            let pattern = format!("{}:queue:*{}", self.options.key_prefix, suffix);
            let keys = self.scan_match(&pattern).await?;
            let prefix = format!("{}:queue:", self.options.key_prefix);
            for key in keys {
                if let Some(rest) = key.strip_prefix(&prefix)
                    && let Some(name) = rest.strip_suffix(*suffix)
                {
                    queues.insert(name.to_string());
                }
            }
        }

        Ok(queues.into_iter().collect())
    }

    /// Retry a failed job
    pub async fn retry_job(&self, job_id: &JobId, queue_name: &str) -> Result<()> {
        let mut metadata = match self.get_job(job_id).await? {
            Some(m) => m,
            None => return Err(ChainMQError::Worker("Job not found".to_string())),
        };

        let mut conn = self.async_conn.clone();
        let job_key = self.job_key(job_id);
        let failed_key = self.failed_key(queue_name);
        let wait_key = self.wait_key(queue_name);

        let _: () = conn
            .lrem::<_, _, ()>(&failed_key, 0, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .lpush::<_, _, ()>(&wait_key, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;

        metadata.state = JobState::Waiting;
        metadata.last_error = None;
        metadata.failed_at = None;
        let metadata_json = serde_json::to_string(&metadata)?;
        let _: () = conn
            .hset::<_, _, _, ()>(&job_key, "metadata", metadata_json)
            .await
            .map_err(ChainMQError::Redis)?;

        Ok(())
    }

    /// Delete a job
    pub async fn delete_job(&self, job_id: &JobId, queue_name: &str) -> Result<()> {
        let mut conn = self.async_conn.clone();
        let job_key = self.job_key(job_id);

        let wait_key = self.wait_key(queue_name);
        let active_key = self.active_key(queue_name);
        let delayed_key = self.delayed_key(queue_name);
        let failed_key = self.failed_key(queue_name);
        let completed_key = self.completed_key(queue_name);

        let _: () = conn
            .lrem::<_, _, ()>(&wait_key, 0, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .srem::<_, _, ()>(&active_key, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .zrem::<_, _, ()>(&delayed_key, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .lrem::<_, _, ()>(&failed_key, 0, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .lrem::<_, _, ()>(&completed_key, 0, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;

        let logs_key = self.job_logs_key(job_id);
        let _: () = conn.del(&job_key).await.map_err(ChainMQError::Redis)?;
        let _: () = conn.del(&logs_key).await.map_err(ChainMQError::Redis)?;

        Ok(())
    }

    /// Append one log line for a job (Redis list, capped at [`QueueOptions::job_logs_max_lines`]).
    pub async fn append_job_log_line(&self, job_id: &JobId, line: JobLogLine) -> Result<()> {
        let mut conn = self.async_conn.clone();
        let key = self.job_logs_key(job_id);
        let json = serde_json::to_string(&line)?;
        let _: () = conn
            .rpush::<_, _, ()>(&key, json)
            .await
            .map_err(ChainMQError::Redis)?;
        let max = self.options.job_logs_max_lines.max(1) as isize;
        let _: () = conn
            .ltrim(&key, -max, -1)
            .await
            .map_err(ChainMQError::Redis)?;
        Ok(())
    }

    /// Read the last `limit` log lines for a job (newest at the end of the returned vec).
    pub async fn get_job_logs(&self, job_id: &JobId, limit: usize) -> Result<Vec<JobLogLine>> {
        let mut conn = self.async_conn.clone();
        let key = self.job_logs_key(job_id);
        let take = limit.clamp(1, 10_000) as isize;
        let raw: Vec<String> = conn
            .lrange(&key, -take, -1)
            .await
            .map_err(ChainMQError::Redis)?;
        let mut out = Vec::with_capacity(raw.len());
        for s in raw {
            if let Ok(line) = serde_json::from_str::<JobLogLine>(&s) {
                out.push(line);
            }
        }
        Ok(out)
    }

    // Redis key helpers
    fn job_key(&self, job_id: &JobId) -> String {
        format!("{}:job:{}", self.options.key_prefix, job_id)
    }

    fn job_logs_key(&self, job_id: &JobId) -> String {
        format!("{}:job:{}:logs", self.options.key_prefix, job_id)
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

    fn completed_key(&self, queue_name: &str) -> String {
        format!("{}:queue:{}:completed", self.options.key_prefix, queue_name)
    }

    /// Recover stalled jobs (jobs in active state for too long)
    pub async fn recover_stalled_jobs(
        &self,
        queue_name: &str,
        max_stalled_secs: u64,
    ) -> Result<usize> {
        let mut conn = self.async_conn.clone();
        let active_key = self.active_key(queue_name);
        let wait_key = self.wait_key(queue_name);

        let active_job_ids: Vec<String> = conn
            .smembers(&active_key)
            .await
            .map_err(ChainMQError::Redis)?;
        let now = Utc::now();
        let mut recovered_count = 0;

        for job_id_str in active_job_ids {
            if let Ok(job_id) = job_id_str.parse::<uuid::Uuid>() {
                let job_id = JobId(job_id);

                if let Ok(Some(metadata)) = self.get_job(&job_id).await {
                    let should_recover = if let Some(started_at) = metadata.started_at {
                        let elapsed = now.signed_duration_since(started_at);

                        let timeout_secs = metadata
                            .options
                            .timeout_secs
                            .unwrap_or(max_stalled_secs)
                            .min(max_stalled_secs);
                        let timeout_duration = chrono::Duration::seconds(timeout_secs as i64);

                        let is_stalled = elapsed > timeout_duration;
                        if is_stalled {
                            tracing::warn!(
                                "Job {} has been active for {:?}, exceeding timeout of {:?}",
                                job_id,
                                elapsed,
                                timeout_duration
                            );
                        }
                        is_stalled
                    } else {
                        tracing::warn!(
                            "Job {} is in active set but has no started_at timestamp",
                            job_id
                        );
                        true
                    };

                    if should_recover {
                        let mut update_con = self.async_conn.clone();

                        let removed: i32 = update_con
                            .srem::<_, _, i32>(&active_key, job_id_str.clone())
                            .await
                            .map_err(ChainMQError::Redis)?;
                        if removed == 0 {
                            tracing::warn!(
                                "Job {} was not in active set (may have been recovered already)",
                                job_id
                            );
                            continue;
                        }

                        let _: () = update_con
                            .lpush::<_, _, ()>(&wait_key, job_id_str.clone())
                            .await
                            .map_err(ChainMQError::Redis)?;

                        let mut updated_metadata = metadata;
                        updated_metadata.state = JobState::Waiting;
                        updated_metadata.started_at = None;
                        updated_metadata.worker_id = None;
                        self.save_job_metadata(&job_id, &updated_metadata).await?;

                        recovered_count += 1;
                        tracing::info!("Recovered job {} - moved from active to waiting", job_id);
                    }
                } else {
                    tracing::warn!(
                        "Job {} found in active set but metadata not found - removing from active set",
                        job_id
                    );
                    let mut update_con = self.async_conn.clone();
                    let _: () = update_con
                        .srem::<_, _, ()>(&active_key, job_id_str.clone())
                        .await
                        .map_err(ChainMQError::Redis)?;
                }
            }
        }

        Ok(recovered_count)
    }
}

#[derive(Debug, Clone)]
pub struct QueueStats {
    pub waiting: usize,
    pub active: usize,
    pub delayed: usize,
    pub failed: usize,
    pub completed: usize,
}
