//! Integration tests against a real Redis.
//!
//! ```text
//! cargo test --test queue_integration -- --ignored
//! ```
//!
//! With Redis at `REDIS_URL` or default `redis://127.0.0.1:6379`.
use async_trait::async_trait;
use chainmq::{
    ChainMQError, Job, JobContext, JobId, JobLogLine, JobMetadata, JobOptions, JobState, Queue,
    QueueOptions, RedisClient, Result,
};
use serde::{Deserialize, Serialize};

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

    queue.fail_job(&id, "boom").await.expect("fail");

    let delayed = queue.get_job(&id).await.expect("get").expect("some");
    assert_eq!(delayed.state, JobState::Delayed);
    assert_eq!(delayed.attempts, 1);

    queue
        .claim_job("integration_q", "w1")
        .await
        .expect("claim2");

    queue.fail_job(&id, "boom2").await.expect("fail2");

    let failed = queue.get_job(&id).await.expect("get").expect("some");
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.attempts, 2);
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
