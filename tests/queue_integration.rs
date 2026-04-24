//! Integration tests against a real Redis.
//!
//! ```text
//! cargo test --test queue_integration -- --ignored
//! ```
//!
//! With Redis at `REDIS_URL` or default `redis://127.0.0.1:6379`.
use async_trait::async_trait;
use chainmq::{
    ChainMQError, Job, JobContext, JobId, JobLogLine, JobMetadata, JobOptions, JobState, Priority,
    Queue, QueueOptions, RedisClient, Result,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Default)]
struct EmptyApp;

impl chainmq::AppContext for EmptyApp {
    fn clone_context(&self) -> Arc<dyn chainmq::AppContext> {
        Arc::new(self.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestJob {
    v: u32,
}

#[async_trait]
impl Job for TestJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        Ok(())
    }

    fn name() -> &'static str {
        "TestJob"
    }

    fn queue_name() -> &'static str {
        "integration_q"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProgressJob {
    v: u32,
}

#[async_trait]
impl Job for ProgressJob {
    async fn perform(&self, ctx: &JobContext) -> Result<()> {
        ctx.set_progress(serde_json::json!({ "pct": 42, "step": "x" }))
            .await?;
        Ok(())
    }

    fn name() -> &'static str {
        "ProgressJob"
    }

    fn queue_name() -> &'static str {
        "integration_q"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ParentJob;

#[async_trait]
impl Job for ParentJob {
    async fn perform(&self, ctx: &JobContext) -> Result<()> {
        let _ = ctx.queue().enqueue(TestJob { v: 99 }).await?;
        Ok(())
    }

    fn name() -> &'static str {
        "ParentJob"
    }

    fn queue_name() -> &'static str {
        "integration_q"
    }
}

fn opts() -> QueueOptions {
    QueueOptions {
        redis: RedisClient::Url(
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6370".into()),
        ),
        key_prefix: format!("cmq_{}", uuid::Uuid::new_v4().as_simple()),
        ..Default::default()
    }
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn enqueue_claim_complete_metadata_roundtrip() {
    let queue = Queue::new(opts()).await.expect("queue new");
    let job = TestJob { v: 42 };
    let id = queue.enqueue(job).await.expect("enqueue");

    let claimed = queue
        .claim_job("integration_q", "worker-a")
        .await
        .expect("claim");
    assert_eq!(claimed, Some(id.clone()));

    let meta: JobMetadata = queue.get_job(&id).await.expect("get").expect("some");
    assert_eq!(meta.state, JobState::Active);
    assert_eq!(meta.worker_id.as_deref(), Some("worker-a"));
    assert!(meta.started_at.is_some());

    queue
        .complete_job(&id, "integration_q", None)
        .await
        .expect("complete");

    let done = queue.get_job(&id).await.expect("get").expect("some");
    assert_eq!(done.state, JobState::Completed);
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn enqueue_custom_job_id_rejected_if_duplicate() {
    let queue = Queue::new(opts()).await.expect("queue new");
    let fixed = JobId::new();
    let mut opts = JobOptions::default();
    opts.job_id = Some(fixed.clone());
    let id = queue
        .enqueue_with_options(TestJob { v: 10 }, opts.clone())
        .await
        .expect("first enqueue");
    assert_eq!(id, fixed);

    let m = queue.get_job(&fixed).await.expect("get").expect("some");
    assert_eq!(m.id, fixed);
    assert!(m.options.job_id.is_none());

    let err = queue
        .enqueue_with_options(TestJob { v: 11 }, opts)
        .await
        .expect_err("duplicate id");
    match err {
        ChainMQError::DuplicateJobId(id) => assert_eq!(id, fixed),
        other => panic!("expected DuplicateJobId, got {:?}", other),
    }
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn fail_job_retries_then_failed() {
    let queue = Queue::new(opts()).await.expect("queue new");
    let job = TestJob { v: 1 };
    let mut opts = JobOptions::default();
    opts.attempts = 2;
    let id = queue
        .enqueue_with_options(job, opts)
        .await
        .expect("enqueue");

    queue.claim_job("integration_q", "w1").await.expect("claim");

    queue
        .fail_after_perform_error(&id, "boom")
        .await
        .expect("fail");

    let delayed = queue.get_job(&id).await.expect("get").expect("some");
    assert_eq!(delayed.state, JobState::Delayed);
    assert_eq!(delayed.attempts, 1);

    queue
        .claim_job("integration_q", "w1")
        .await
        .expect("claim2");

    queue
        .fail_after_perform_error(&id, "boom2")
        .await
        .expect("fail2");

    let failed = queue.get_job(&id).await.expect("get").expect("some");
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.attempts, 2);
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn fail_job_without_claim_removes_from_wait() {
    let queue = Queue::new(opts()).await.expect("queue new");
    let mut opts = JobOptions::default();
    opts.attempts = 1;
    let id = queue
        .enqueue_with_options(TestJob { v: 7 }, opts)
        .await
        .expect("enqueue");

    queue.fail_job(&id, "pre-run fail").await.expect("fail");

    let m = queue.get_job(&id).await.expect("get").expect("some");
    assert_eq!(m.state, JobState::Failed);
    assert_eq!(m.attempts, 1);

    let claimed = queue.claim_job("integration_q", "w").await.expect("claim");
    assert!(
        claimed.is_none(),
        "job must not remain on wait after fail_job"
    );
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn fail_job_moves_completed_job_to_failed() {
    let queue = Queue::new(opts()).await.expect("queue new");
    let id = queue.enqueue(TestJob { v: 21 }).await.expect("enqueue");
    queue.claim_job("integration_q", "w").await.expect("claim");
    queue
        .complete_job(&id, "integration_q", None)
        .await
        .expect("complete");

    queue
        .fail_job(&id, "admin revoked send")
        .await
        .expect("fail after complete");

    let m = queue.get_job(&id).await.expect("get").expect("some");
    assert_eq!(m.state, JobState::Failed);
    assert_eq!(m.last_error.as_deref(), Some("admin revoked send"));
    assert!(m.completed_at.is_none());
    assert!(m.response.is_none());
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn process_delayed_moves_to_waiting() {
    let queue = Queue::new(opts()).await.expect("queue new");
    let job = TestJob { v: 2 };
    let mut jo = JobOptions::default();
    jo.delay_secs = Some(0);
    let id = queue.enqueue_with_options(job, jo).await.expect("enqueue");

    let n = queue
        .process_delayed("integration_q")
        .await
        .expect("process_delayed");
    assert!(n >= 1);

    let m = queue.get_job(&id).await.expect("get").expect("some");
    assert_eq!(m.state, JobState::Waiting);
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn requeue_claimed_job() {
    let queue = Queue::new(opts()).await.expect("queue new");
    let id = queue.enqueue(TestJob { v: 3 }).await.expect("enqueue");
    queue.claim_job("integration_q", "w").await.expect("claim");

    queue
        .requeue_claimed_job(&id, "integration_q")
        .await
        .expect("requeue");

    let m = queue.get_job(&id).await.expect("get").expect("some");
    assert_eq!(m.state, JobState::Waiting);
    assert!(m.started_at.is_none());
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn list_queues_uses_scan_and_registry() {
    let queue = Queue::new(opts()).await.expect("queue new");
    queue.enqueue(TestJob { v: 4 }).await.expect("enqueue");
    let names = queue.list_queues().await.expect("list_queues");
    assert!(
        names.iter().any(|n| n == "integration_q"),
        "expected integration_q in {:?}",
        names
    );
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn job_logs_append_and_read_roundtrip() {
    let queue = Queue::new(opts()).await.expect("queue new");
    let id = queue.enqueue(TestJob { v: 5 }).await.expect("enqueue");
    let line = JobLogLine {
        ts: "2026-01-01T00:00:00.000Z".to_string(),
        level: "INFO".to_string(),
        message: "integration test line".to_string(),
    };
    queue
        .append_job_log_line(&id, line.clone())
        .await
        .expect("append");
    let lines = queue.get_job_logs(&id, 50).await.expect("get logs");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].message, line.message);
    assert_eq!(lines[0].level, line.level);

    queue
        .delete_job(&id, "integration_q")
        .await
        .expect("delete");
    let after = queue.get_job_logs(&id, 50).await.expect("get after del");
    assert!(after.is_empty());
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn claim_prefers_higher_priority_first() {
    let queue = Queue::new(opts()).await.expect("queue new");
    let mut lo = JobOptions::default();
    lo.priority = Priority::Low;
    let low_id = queue
        .enqueue_with_options(TestJob { v: 1 }, lo)
        .await
        .expect("enqueue low");

    let mut hi = JobOptions::default();
    hi.priority = Priority::High;
    let hi_id = queue
        .enqueue_with_options(TestJob { v: 2 }, hi)
        .await
        .expect("enqueue high");

    let first = queue.claim_job("integration_q", "w").await.expect("claim");
    assert_eq!(first, Some(hi_id));
    let second = queue.claim_job("integration_q", "w").await.expect("claim2");
    assert_eq!(second, Some(low_id));
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn lifo_bucket_claims_newer_first() {
    let queue = Queue::new(opts()).await.expect("queue new");
    let mut fifo = JobOptions::default();
    fifo.priority = Priority::Normal;
    fifo.lifo = false;
    let a = queue
        .enqueue_with_options(TestJob { v: 1 }, fifo.clone())
        .await
        .expect("a");
    let b = queue
        .enqueue_with_options(TestJob { v: 2 }, fifo)
        .await
        .expect("b");

    let first = queue.claim_job("integration_q", "w").await.expect("claim");
    assert_eq!(first, Some(a.clone()), "FIFO should take oldest first");

    queue
        .complete_job(&a, "integration_q", None)
        .await
        .expect("complete a");
    queue
        .complete_job(&b, "integration_q", None)
        .await
        .expect("complete b");

    let mut lifo_opts = JobOptions::default();
    lifo_opts.priority = Priority::Normal;
    lifo_opts.lifo = true;
    let x = queue
        .enqueue_with_options(TestJob { v: 3 }, lifo_opts.clone())
        .await
        .expect("x");
    let y = queue
        .enqueue_with_options(TestJob { v: 4 }, lifo_opts)
        .await
        .expect("y");

    let first_l = queue.claim_job("integration_q", "w").await.expect("claim");
    assert_eq!(
        first_l,
        Some(y),
        "LIFO bucket should take newest (last enqueued) first"
    );
    let second_l = queue.claim_job("integration_q", "w").await.expect("c2");
    assert_eq!(second_l, Some(x));
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn perform_can_enqueue_follow_up_via_context_queue() {
    let queue = Arc::new(Queue::new(opts()).await.expect("queue new"));
    let parent_id = queue.enqueue(ParentJob).await.expect("parent");

    let meta = queue.get_job(&parent_id).await.expect("get").expect("meta");
    let job_ctx = JobContext::new(
        parent_id.clone(),
        meta,
        Arc::new(EmptyApp::default()) as Arc<dyn chainmq::AppContext>,
        Arc::clone(&queue),
        chainmq::CancellationToken::new(),
    );
    ParentJob.perform(&job_ctx).await.expect("perform");

    let waiting = queue
        .list_jobs("integration_q", JobState::Waiting, Some(10))
        .await
        .expect("list");
    assert!(
        waiting
            .iter()
            .any(|j| j.name == "TestJob" && j.payload["v"] == 99),
        "expected child TestJob on wait: {:?}",
        waiting
    );
}

#[tokio::test]
#[ignore = "requires Redis; run: cargo test --test queue_integration -- --ignored"]
async fn set_progress_persisted_on_metadata() {
    let queue = Arc::new(Queue::new(opts()).await.expect("queue new"));
    let id = queue.enqueue(ProgressJob { v: 1 }).await.expect("enqueue");

    let meta = queue.get_job(&id).await.expect("get").expect("meta");
    let job_ctx = JobContext::new(
        id.clone(),
        meta,
        Arc::new(EmptyApp::default()) as Arc<dyn chainmq::AppContext>,
        Arc::clone(&queue),
        chainmq::CancellationToken::new(),
    );
    ProgressJob { v: 1 }
        .perform(&job_ctx)
        .await
        .expect("perform");

    let m = queue.get_job(&id).await.expect("get2").expect("some");
    assert_eq!(
        m.progress,
        Some(serde_json::json!({ "pct": 42, "step": "x" }))
    );
}
