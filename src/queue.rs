use crate::redis::RedisClient;
// src/queue.rs
use crate::{
    ChainMQError, Job, JobId, JobLogLine, JobMetadata, JobOptions, JobState, Priority, Result,
    lua::LuaScripts,
};
use chrono::Utc;
use redis::AsyncCommands;
use serde_json;
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use redis::aio::ConnectionManager;

/// Options for queue configuration
#[derive(Debug, Clone)]
pub struct QueueOptions {
    pub name: String,
    pub redis: RedisClient,
    pub key_prefix: String,
    pub default_concurrency: usize,
    pub max_stalled_interval: u64,
    /// Max log lines retained per job in Redis (older lines dropped). Used by job log tracing layer.
    pub job_logs_max_lines: usize,
    /// Max entries kept on the completed list after each completion. `None` disables trimming.
    pub max_completed_len: Option<usize>,
    /// Max entries kept on the failed list after each terminal failure. `None` disables trimming.
    pub max_failed_len: Option<usize>,
    /// Approximate max entries per logical queue events stream (`XADD MAXLEN ~`).
    pub events_stream_max_len: usize,
}

impl Default for QueueOptions {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            redis: RedisClient::Url("redis://127.0.0.1:6379".into()),
            key_prefix: "rbq".to_string(),
            default_concurrency: 10,
            max_stalled_interval: 30000, // 30 seconds
            job_logs_max_lines: 500,
            max_completed_len: Some(1000),
            max_failed_len: None,
            events_stream_max_len: 1000,
        }
    }
}

/// Redis-backed job queue
pub struct Queue {
    options: QueueOptions,
    scripts: LuaScripts,
    async_conn: ConnectionManager,
}

impl Queue {
    pub async fn new(options: QueueOptions) -> Result<Self> {
        let async_conn = match &options.redis {
            RedisClient::Manager(manager) => manager.clone(),
            RedisClient::Url(url) => RedisClient::build_connection_manager_from_url(url).await?,
            RedisClient::Client(client) => {
                RedisClient::build_connection_manager_from_client(client.clone()).await?
            }
        };

        let scripts = LuaScripts::new();

        Ok(Self {
            options,
            scripts,
            async_conn,
        })
    }

    fn queues_registry_key(&self) -> String {
        format!("{}:queues", self.options.key_prefix)
    }

    fn events_channel(&self, queue_name: &str) -> String {
        format!("{}:events:{}", self.options.key_prefix, queue_name)
    }

    fn events_stream_key(&self, queue_name: &str) -> String {
        format!(
            "{}:events:{}:stream",
            self.options.key_prefix, queue_name
        )
    }

    /// Redis pub/sub channel for this logical queue (same payload as the events stream).
    pub fn redis_events_channel(&self, queue_name: &str) -> String {
        self.events_channel(queue_name)
    }

    /// Open a [`redis::Client`] for pub/sub (live events). Unsupported when the queue was built with [`RedisClient::Manager`].
    pub fn redis_pubsub_client(&self) -> Result<redis::Client> {
        match &self.options.redis {
            crate::redis::RedisClient::Url(u) => Ok(redis::Client::open(u.as_str())?),
            crate::redis::RedisClient::Client(c) => Ok(c.clone()),
            crate::redis::RedisClient::Manager(_) => Err(ChainMQError::Worker(
                "Redis ConnectionManager mount does not support pub/sub; use RedisClient::Url or Client for live queue events.".into(),
            )),
        }
    }

    /// Append to the capped stream and publish the same JSON to the queue events channel.
    pub async fn emit_queue_event(
        &self,
        queue_name: &str,
        event_type: &str,
        job_id: Option<&JobId>,
        data: Option<serde_json::Value>,
        detail: Option<&str>,
    ) {
        let ts = Utc::now().timestamp_millis();
        let mut body = serde_json::json!({
            "type": event_type,
            "ts": ts,
        });
        if let Some(j) = job_id {
            body["jobId"] = serde_json::Value::String(j.to_string());
        }
        if let Some(d) = detail {
            body["detail"] = serde_json::Value::String(d.to_string());
        }
        if let Some(data) = data {
            body["data"] = data;
        }
        let payload = match serde_json::to_string(&body) {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(error = %e, "queue event serialize failed");
                return;
            }
        };

        let stream_key = self.events_stream_key(queue_name);
        let mut conn = self.async_conn.clone();
        let maxlen = self.options.events_stream_max_len;
        let xadd_res = if maxlen > 0 {
            redis::cmd("XADD")
                .arg(&stream_key)
                .arg("MAXLEN")
                .arg("~")
                .arg(maxlen)
                .arg("*")
                .arg("payload")
                .arg(&payload)
                .query_async::<String>(&mut conn)
                .await
        } else {
            redis::cmd("XADD")
                .arg(&stream_key)
                .arg("*")
                .arg("payload")
                .arg(&payload)
                .query_async::<String>(&mut conn)
                .await
        };
        if let Err(e) = xadd_res {
            tracing::debug!(error = %e, "queue event XADD failed");
        }

        if let Err(e) = conn
            .publish::<_, _, ()>(self.events_channel(queue_name), &payload)
            .await
        {
            tracing::debug!(error = %e, "queue event publish failed");
        }
    }

    /// Recent queue lifecycle events (newest first), parsed from the events stream.
    pub async fn read_queue_events(
        &self,
        queue_name: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let take = limit.clamp(1, 500);
        let stream_key = self.events_stream_key(queue_name);
        let mut conn = self.async_conn.clone();
        let raw: Vec<(String, Vec<(String, String)>)> = redis::cmd("XREVRANGE")
            .arg(&stream_key)
            .arg("+")
            .arg("-")
            .arg("COUNT")
            .arg(take)
            .query_async(&mut conn)
            .await
            .map_err(ChainMQError::Redis)?;

        let mut out = Vec::with_capacity(raw.len());
        for (_id, fields) in raw {
            for (k, v) in fields {
                if k == "payload" {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&v) {
                        out.push(val);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Whitelisted `INFO` fields for the dashboard (shared Redis instance).
    pub async fn redis_server_metrics_json(&self) -> Result<serde_json::Value> {
        let mut conn = self.async_conn.clone();
        let info: String = redis::cmd("INFO")
            .arg("server")
            .arg("memory")
            .arg("clients")
            .arg("stats")
            .query_async(&mut conn)
            .await
            .map_err(ChainMQError::Redis)?;
        Ok(parse_redis_info_whitelist(&info))
    }

    /// Claim order: higher priority first (discriminants 20,10,5,1); FIFO bucket then LIFO per level; then legacy `:wait`.
    const WAIT_BUCKET_ORDER: [(i32, bool); 8] = [
        (20, false),
        (20, true),
        (10, false),
        (10, true),
        (5, false),
        (5, true),
        (1, false),
        (1, true),
    ];

    fn wait_stream_key(&self, queue_name: &str, disc: i32, lifo: bool) -> String {
        if lifo {
            format!(
                "{}:queue:{}:waitl:p{}",
                self.options.key_prefix, queue_name, disc
            )
        } else {
            format!(
                "{}:queue:{}:wait:p{}",
                self.options.key_prefix, queue_name, disc
            )
        }
    }

    fn wait_legacy_key(&self, queue_name: &str) -> String {
        format!("{}:queue:{}:wait", self.options.key_prefix, queue_name)
    }

    fn claim_wait_key_chain(&self, queue_name: &str) -> Vec<String> {
        let mut v = Vec::with_capacity(9);
        for &(disc, lifo) in &Self::WAIT_BUCKET_ORDER {
            v.push(self.wait_stream_key(queue_name, disc, lifo));
        }
        v.push(self.wait_legacy_key(queue_name));
        v
    }

    async fn push_job_to_wait(
        &self,
        conn: &mut ConnectionManager,
        queue_name: &str,
        job_id_str: &str,
        disc: i32,
        lifo: bool,
    ) -> Result<()> {
        let key = self.wait_stream_key(queue_name, disc, lifo);
        if lifo {
            let _: () = conn
                .rpush::<_, _, ()>(&key, job_id_str)
                .await
                .map_err(ChainMQError::Redis)?;
        } else {
            let _: () = conn
                .lpush::<_, _, ()>(&key, job_id_str)
                .await
                .map_err(ChainMQError::Redis)?;
        }
        Ok(())
    }

    async fn lrem_from_all_wait_streams(
        &self,
        conn: &mut ConnectionManager,
        queue_name: &str,
        id_str: &str,
    ) -> Result<()> {
        for k in self.claim_wait_key_chain(queue_name) {
            let _: () = conn
                .lrem::<_, _, ()>(&k, 0, id_str)
                .await
                .map_err(ChainMQError::Redis)?;
        }
        Ok(())
    }

    pub(crate) async fn save_job_metadata(
        &self,
        job_id: &JobId,
        metadata: &JobMetadata,
    ) -> Result<()> {
        let mut conn = self.async_conn.clone();
        let job_key = self.job_key(job_id);
        let json = serde_json::to_string(metadata)?;
        let disc = metadata.options.priority.redis_discriminant();
        let lifo = if metadata.options.lifo { "1" } else { "0" };
        redis::pipe()
            .atomic()
            .hset(&job_key, "metadata", json.as_str())
            .hset(&job_key, "priority", disc)
            .hset(&job_key, "enqueue_lifo", lifo)
            .query_async::<()>(&mut conn)
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
        self.save_job_metadata(job_id, &metadata).await?;
        let qn = metadata.queue_name.clone();
        self.emit_queue_event(&qn, "active", Some(job_id), None, None)
            .await;
        Ok(())
    }

    /// Return a claimed job to the waiting queue (e.g. worker shutdown before execution).
    pub async fn requeue_claimed_job(&self, job_id: &JobId, queue_name: &str) -> Result<()> {
        let mut conn = self.async_conn.clone();
        let active_key = self.active_key(queue_name);

        let _: i32 = conn
            .srem::<_, _, i32>(&active_key, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;

        let id_str = job_id.to_string();
        let (disc, lifo) = if let Some(m) = self.get_job(job_id).await? {
            (m.options.priority.redis_discriminant(), m.options.lifo)
        } else {
            (Priority::Normal.redis_discriminant(), false)
        };
        self.push_job_to_wait(&mut conn, queue_name, &id_str, disc, lifo)
            .await?;

        if let Some(mut metadata) = self.get_job(job_id).await? {
            metadata.state = JobState::Waiting;
            metadata.started_at = None;
            metadata.worker_id = None;
            self.save_job_metadata(job_id, &metadata).await?;
        }

        self.emit_queue_event(queue_name, "waiting", Some(job_id), None, None)
            .await;

        Ok(())
    }

    /// Enqueue a job with default options
    pub async fn enqueue<T: Job>(&self, job: T) -> Result<JobId> {
        self.enqueue_with_options(job, T::default_options()).await
    }

    /// Enqueue a job with custom options.
    ///
    /// If [`JobOptions::job_id`] is set, that id is used; otherwise a new UUID is generated.
    /// If a job hash already has `metadata` for that id, returns [`ChainMQError::DuplicateJobId`]
    /// (atomic via Redis `HSETNX`). After [`Queue::delete_job`], the same id may be enqueued again.
    pub async fn enqueue_with_options<T: Job>(&self, job: T, options: JobOptions) -> Result<JobId> {
        let job_id = options.job_id.clone().unwrap_or_else(JobId::new);
        let now = Utc::now();

        let mut stored_options = options.clone();
        stored_options.job_id = None;

        let metadata = JobMetadata {
            id: job_id.clone(),
            name: T::name().to_string(),
            queue_name: T::queue_name().to_string(),
            payload: serde_json::to_value(&job)?,
            options: stored_options,
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
            progress: None,
        };

        let mut conn = self.async_conn.clone();
        let job_key = self.job_key(&job_id);
        let metadata_json = serde_json::to_string(&metadata)?;
        let inserted: bool = conn
            .hset_nx(&job_key, "metadata", &metadata_json)
            .await
            .map_err(ChainMQError::Redis)?;
        if !inserted {
            return Err(ChainMQError::DuplicateJobId(job_id));
        }

        self.save_job_metadata(&job_id, &metadata).await?;

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
            self.emit_queue_event(
                qname,
                "delayed",
                Some(&job_id),
                Some(serde_json::json!({ "executeAt": execute_at })),
                None,
            )
            .await;
        } else {
            let disc = metadata.options.priority.redis_discriminant();
            let lifo = metadata.options.lifo;
            self.push_job_to_wait(&mut conn, qname, &job_id.to_string(), disc, lifo)
                .await?;
            self.emit_queue_event(qname, "waiting", Some(&job_id), None, None)
                .await;
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
                self.emit_queue_event(
                    queue_name,
                    "delayed_moved",
                    Some(&job_id),
                    None,
                    None,
                )
                .await;
            }
        }

        Ok(ids.len())
    }

    /// Claim next job for processing
    pub async fn claim_job(&self, queue_name: &str, worker_id: &str) -> Result<Option<JobId>> {
        let chain = self.claim_wait_key_chain(queue_name);
        let active_key = self.active_key(queue_name);

        let mut inv = self.scripts.claim_job.key(&chain[0]);
        for k in chain.iter().skip(1) {
            inv.key(k);
        }
        inv.key(&active_key);
        let result: Option<String> = inv.invoke_async(&mut self.async_conn.clone()).await?;

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
    ///
    /// If the job is already [`JobState::Failed`] or [`JobState::Completed`], returns `Ok` after
    /// best-effort removal from the active set only. This avoids worker errors when
    /// [`Self::fail_job`] or another path finished the job while the worker was still running `perform`.
    ///
    /// Otherwise removes the id from wait, active, delayed, failed, and completed lists for
    /// `queue_name` before recording completion so delayed or waiting jobs can be completed
    /// without leaving stale ids in the delayed zset or wait list. Clears [`crate::JobMetadata::last_error`]
    /// and [`crate::JobMetadata::failed_at`] so a successful completion does not show stale errors in the UI.
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
        let active_key = self.active_key(queue_name);
        let id_str = job_id.to_string();

        if matches!(metadata.state, JobState::Failed | JobState::Completed) {
            let _: () = conn
                .srem::<_, _, ()>(&active_key, &id_str)
                .await
                .map_err(ChainMQError::Redis)?;
            return Ok(());
        }

        let job_key = self.job_key(job_id);
        let delayed_key = self.delayed_key(queue_name);
        let failed_key = self.failed_key(queue_name);
        let completed_key = self.completed_key(queue_name);

        self.lrem_from_all_wait_streams(&mut conn, queue_name, &id_str)
            .await?;
        let _: () = conn
            .srem::<_, _, ()>(&active_key, &id_str)
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .zrem::<_, _, ()>(&delayed_key, &id_str)
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .lrem::<_, _, ()>(&failed_key, 0, &id_str)
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .lrem::<_, _, ()>(&completed_key, 0, &id_str)
            .await
            .map_err(ChainMQError::Redis)?;

        let now = Utc::now();
        metadata.state = JobState::Completed;
        metadata.completed_at = Some(now);
        metadata.response = response;
        // Clear any stale failure fields (e.g. mistaken fail_job before run, or after retries).
        metadata.last_error = None;
        metadata.failed_at = None;

        let metadata_json = serde_json::to_string(&metadata)?;
        let _: () = conn
            .hset::<_, _, _, ()>(&job_key, "metadata", metadata_json)
            .await
            .map_err(ChainMQError::Redis)?;

        let _: () = conn
            .lpush::<_, _, ()>(&completed_key, &id_str)
            .await
            .map_err(ChainMQError::Redis)?;

        if let Some(max) = self.options.max_completed_len {
            if max > 0 {
                let _: () = conn
                    .ltrim(&completed_key, 0, (max as isize).saturating_sub(1))
                    .await
                    .map_err(ChainMQError::Redis)?;
            }
        }

        self.emit_queue_event(queue_name, "completed", Some(job_id), None, None)
            .await;

        Ok(())
    }

    /// Remove `id_str` from wait, active, delayed, failed, and completed structures for `queue_name`.
    async fn detach_job_from_all_queue_lists(
        &self,
        conn: &mut ConnectionManager,
        queue_name: &str,
        id_str: &str,
    ) -> Result<()> {
        let active_key = self.active_key(queue_name);
        let delayed_key = self.delayed_key(queue_name);
        let failed_key = self.failed_key(queue_name);
        let completed_key = self.completed_key(queue_name);

        self.lrem_from_all_wait_streams(conn, queue_name, id_str)
            .await?;
        let _: () = conn
            .srem::<_, _, ()>(&active_key, id_str)
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .zrem::<_, _, ()>(&delayed_key, id_str)
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .lrem::<_, _, ()>(&failed_key, 0, id_str)
            .await
            .map_err(ChainMQError::Redis)?;
        let _: () = conn
            .lrem::<_, _, ()>(&completed_key, 0, id_str)
            .await
            .map_err(ChainMQError::Redis)?;
        Ok(())
    }

    /// Record a failed `perform` and either schedule a retry (delayed) or move to the failed list.
    /// The worker calls this when `perform` returns `Err`. For a **terminal** failure from your
    /// own code (for example right after enqueue), use [`Queue::fail_job`] instead.
    pub async fn fail_after_perform_error(&self, job_id: &JobId, error: &str) -> Result<()> {
        let mut meta = match self.get_job(job_id).await? {
            Some(m) => m,
            None => return Err(ChainMQError::JobNotFound(job_id.clone())),
        };

        let queue_name = meta.queue_name.clone();
        let mut conn = self.async_conn.clone();
        let id_str = job_id.to_string();
        self.detach_job_from_all_queue_lists(&mut conn, &queue_name, &id_str)
            .await?;

        let new_attempts = meta.attempts + 1;
        meta.attempts = new_attempts;
        meta.last_error = Some(error.to_string());
        meta.failed_at = Some(Utc::now());

        let delayed_key = self.delayed_key(&queue_name);
        let failed_key = self.failed_key(&queue_name);

        if new_attempts < meta.options.attempts {
            let delay = meta.options.backoff.calculate_delay(new_attempts);
            let execute_at = Utc::now().timestamp() + delay.as_secs() as i64;
            meta.state = JobState::Delayed;

            self.save_job_metadata(job_id, &meta).await?;

            let _: () = conn
                .zadd::<_, _, _, ()>(&delayed_key, &id_str, execute_at)
                .await
                .map_err(ChainMQError::Redis)?;

            self.emit_queue_event(
                &queue_name,
                "delayed",
                Some(job_id),
                Some(serde_json::json!({
                    "executeAt": execute_at,
                    "attempt": new_attempts,
                })),
                None,
            )
            .await;
        } else {
            meta.state = JobState::Failed;
            self.save_job_metadata(job_id, &meta).await?;

            let _: () = conn
                .lpush::<_, _, ()>(&failed_key, &id_str)
                .await
                .map_err(ChainMQError::Redis)?;

            if let Some(max) = self.options.max_failed_len {
                if max > 0 {
                    let _: () = conn
                        .ltrim(&failed_key, 0, (max as isize).saturating_sub(1))
                        .await
                        .map_err(ChainMQError::Redis)?;
                }
            }

            self.emit_queue_event(
                &queue_name,
                "failed",
                Some(job_id),
                None,
                Some(error),
            )
                .await;
        }

        Ok(())
    }

    /// Permanently move a job to the **failed** state and failed list.
    ///
    /// Works from any prior state (waiting, active, delayed, **completed**, or already failed):
    /// clears completion fields, increments attempts, sets `last_error`, and records the job on
    /// the failed queue. Does **not** apply retry/backoff (the worker uses an internal path when
    /// `perform` returns `Err` to schedule retries or terminal failure).
    pub async fn fail_job(&self, job_id: &JobId, error: &str) -> Result<()> {
        let mut meta = match self.get_job(job_id).await? {
            Some(m) => m,
            None => return Err(ChainMQError::JobNotFound(job_id.clone())),
        };

        let queue_name = meta.queue_name.clone();
        let mut conn = self.async_conn.clone();
        let id_str = job_id.to_string();
        self.detach_job_from_all_queue_lists(&mut conn, &queue_name, &id_str)
            .await?;

        meta.attempts = meta.attempts.saturating_add(1);
        meta.last_error = Some(error.to_string());
        meta.failed_at = Some(Utc::now());
        meta.state = JobState::Failed;
        meta.completed_at = None;
        meta.response = None;
        meta.started_at = None;
        meta.worker_id = None;

        let failed_key = self.failed_key(&queue_name);
        self.save_job_metadata(job_id, &meta).await?;
        let _: () = conn
            .lpush::<_, _, ()>(&failed_key, &id_str)
            .await
            .map_err(ChainMQError::Redis)?;

        if let Some(max) = self.options.max_failed_len {
            if max > 0 {
                let _: () = conn
                    .ltrim(&failed_key, 0, (max as isize).saturating_sub(1))
                    .await
                    .map_err(ChainMQError::Redis)?;
            }
        }

        self.emit_queue_event(
            &queue_name,
            "failed",
            Some(job_id),
            None,
            Some(error),
        )
            .await;

        Ok(())
    }

    /// Get queue statistics
    pub async fn get_stats(&self, queue_name: &str) -> Result<QueueStats> {
        let mut conn = self.async_conn.clone();

        let mut wait_len: usize = 0;
        for k in self.claim_wait_key_chain(queue_name) {
            let len: usize = conn.llen(&k).await.map_err(ChainMQError::Redis)?;
            wait_len += len;
        }
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
                let mut collected: Vec<String> = Vec::new();
                for k in self.claim_wait_key_chain(queue_name) {
                    let part: Vec<String> =
                        conn.lrange(&k, 0, -1).await.map_err(ChainMQError::Redis)?;
                    collected.extend(part);
                    if collected.len() >= limit {
                        break;
                    }
                }
                collected.into_iter().take(limit).collect()
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
                        let mut found = false;
                        for k in self.claim_wait_key_chain(queue_name) {
                            let list: Vec<String> = self
                                .async_conn
                                .clone()
                                .lrange(&k, 0, -1)
                                .await
                                .map_err(ChainMQError::Redis)?;
                            if list.contains(&job_id_str) {
                                found = true;
                                break;
                            }
                        }
                        found
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

        for pattern in [
            format!("{}:queue:*:wait:p*", self.options.key_prefix),
            format!("{}:queue:*:waitl:p*", self.options.key_prefix),
        ] {
            let keys = self.scan_match(&pattern).await?;
            let qp = format!("{}:queue:", self.options.key_prefix);
            for key in keys {
                if let Some(rest) = key.strip_prefix(&qp) {
                    if let Some((name, _)) = rest.split_once(":wait:p") {
                        queues.insert(name.to_string());
                    } else if let Some((name, _)) = rest.split_once(":waitl:p") {
                        queues.insert(name.to_string());
                    }
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

        let _: () = conn
            .lrem::<_, _, ()>(&failed_key, 0, job_id.to_string())
            .await
            .map_err(ChainMQError::Redis)?;
        let disc = metadata.options.priority.redis_discriminant();
        let lifo = metadata.options.lifo;
        self.push_job_to_wait(&mut conn, queue_name, &job_id.to_string(), disc, lifo)
            .await?;

        metadata.state = JobState::Waiting;
        metadata.last_error = None;
        metadata.failed_at = None;
        let metadata_json = serde_json::to_string(&metadata)?;
        let _: () = conn
            .hset::<_, _, _, ()>(&job_key, "metadata", metadata_json)
            .await
            .map_err(ChainMQError::Redis)?;

        self.emit_queue_event(queue_name, "retried", Some(job_id), None, None)
            .await;

        Ok(())
    }

    /// Delete a job
    pub async fn delete_job(&self, job_id: &JobId, queue_name: &str) -> Result<()> {
        let mut conn = self.async_conn.clone();
        let job_key = self.job_key(job_id);

        let active_key = self.active_key(queue_name);
        let delayed_key = self.delayed_key(queue_name);
        let failed_key = self.failed_key(queue_name);
        let completed_key = self.completed_key(queue_name);

        self.lrem_from_all_wait_streams(&mut conn, queue_name, &job_id.to_string())
            .await?;
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

        self.emit_queue_event(queue_name, "removed", Some(job_id), None, None)
            .await;

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

                        let disc = metadata.options.priority.redis_discriminant();
                        let lifo = metadata.options.lifo;
                        self.push_job_to_wait(&mut update_con, queue_name, &job_id_str, disc, lifo)
                            .await?;

                        let mut updated_metadata = metadata;
                        updated_metadata.state = JobState::Waiting;
                        updated_metadata.started_at = None;
                        updated_metadata.worker_id = None;
                        self.save_job_metadata(&job_id, &updated_metadata).await?;

                        self.emit_queue_event(queue_name, "stalled", Some(&job_id), None, None)
                            .await;

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

/// Parse `INFO` output and keep only non-sensitive dashboard fields.
fn parse_redis_info_whitelist(info: &str) -> serde_json::Value {
    let allowed: HashSet<&str> = [
        "redis_version",
        "redis_mode",
        "os",
        "uptime_in_seconds",
        "used_memory_human",
        "used_memory",
        "used_memory_rss",
        "maxmemory",
        "maxmemory_human",
        "total_system_memory",
        "mem_fragmentation_ratio",
        "connected_clients",
        "blocked_clients",
        "total_commands_processed",
        "instantaneous_ops_per_sec",
        "keyspace_hits",
        "keyspace_misses",
    ]
    .into_iter()
    .collect();
    let mut map = serde_json::Map::new();
    for line in info.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let key = k.trim();
        if allowed.contains(key) {
            map.insert(
                key.to_string(),
                serde_json::Value::String(v.trim().to_string()),
            );
        }
    }
    serde_json::Value::Object(map)
}

#[derive(Debug, Clone)]
pub struct QueueStats {
    pub waiting: usize,
    pub active: usize,
    pub delayed: usize,
    pub failed: usize,
    pub completed: usize,
}
