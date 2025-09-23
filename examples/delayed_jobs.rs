use chainmq::{
    AppContext, Job, JobContext, JobOptions, JobRegistry, Priority, Queue, QueueOptions, Result,
    WorkerBuilder, async_trait,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
struct NotifyJob {
    user_id: String,
}

#[async_trait]
impl Job for NotifyJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        println!("[NotifyJob] notifying user_id={}", self.user_id);
        Ok(())
    }
    fn name() -> &'static str {
        "NotifyJob"
    }
    fn queue_name() -> &'static str {
        "default"
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
    registry.register::<NotifyJob>();

    let mut worker = WorkerBuilder::new_with_redis_uri("redis://localhost:6379", registry)
        .with_app_context(app)
        .with_queue_name("default")
        .spawn()
        .await?;
    worker.start().await?;

    let queue = Queue::new(QueueOptions::default()).await?;
    let job = NotifyJob {
        user_id: "u123".into(),
    };
    let opts = JobOptions {
        delay_secs: Some(5),
        priority: Priority::Normal,
        ..Default::default()
    };
    let id = queue.enqueue_with_options(job, opts).await?;
    println!("[NotifyJob] enqueued with 5s delay id={}", id);

    tokio::signal::ctrl_c().await?;
    worker.stop().await;
    Ok(())
}
