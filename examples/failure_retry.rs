use chainmq::{
    AppContext, Job, JobContext, JobOptions, JobRegistry, Result, WorkerBuilder, async_trait,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Serialize, Deserialize)]
struct FlakyJob {
    fail_times: u32,
}

#[async_trait]
impl Job for FlakyJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
        let n = ATTEMPTS.fetch_add(1, Ordering::SeqCst) + 1;
        println!("[FlakyJob] attempt {}", n);
        if n <= self.fail_times {
            Err(anyhow::anyhow!("simulated failure").into())
        } else {
            Ok(())
        }
    }
    fn name() -> &'static str {
        "FlakyJob"
    }
}

#[derive(Clone, Default)]
struct AppState;
impl AppContext for AppState {
    fn clone_context(&self) -> Arc<dyn AppContext> {
        Arc::new(self.clone())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::try_init().ok();
    let app = Arc::new(AppState::default());
    let mut registry = JobRegistry::new();
    registry.register::<FlakyJob>();

    let mut worker = WorkerBuilder::new("redis://localhost:6379", registry)
        .with_app_context(app)
        .with_queue_name("default")
        .with_concurrency(1)
        .spawn()
        .await?;
    worker.start().await?;

    // Enqueue a job that fails twice, then succeeds. Backoff and attempts configured.
    let queue = chainmq::Queue::new(chainmq::QueueOptions::default()).await?;
    let job = FlakyJob { fail_times: 2 };
    let opts = JobOptions {
        attempts: 3,
        ..Default::default()
    };
    let id = queue.enqueue_with_options(job, opts).await?;
    println!("[FlakyJob] enqueued id={}", id);

    tokio::signal::ctrl_c().await?;
    worker.stop().await;
    Ok(())
}
