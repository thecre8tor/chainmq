use chainmq::{AppContext, Job, JobContext, JobRegistry, Result, WorkerBuilder, async_trait};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
struct EmailJob {
    to: String,
}

#[derive(Serialize, Deserialize)]
struct ReportJob {
    report_id: u32,
}

#[async_trait]
impl Job for EmailJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        println!("[EmailJob] sending to={}", self.to);
        Ok(())
    }
    fn name() -> &'static str {
        "EmailJob"
    }
    fn queue_name() -> &'static str {
        "default"
    }
}

#[async_trait]
impl Job for ReportJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        println!("[ReportJob] generating report_id={}", self.report_id);
        Ok(())
    }
    fn name() -> &'static str {
        "ReportJob"
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
    registry.register::<EmailJob>();
    registry.register::<ReportJob>();

    let mut worker = WorkerBuilder::new_with_redis_uri(
        option_env!("REDIS_URL").unwrap_or("redis://localhost:6379"),
        registry,
    )
    .with_app_context(app)
    .with_concurrency(4)
    .with_queue_name("default")
    .spawn()
    .await?;

    println!("[worker] multi-jobs-single-worker started on queue 'default'");
    worker.start().await?;

    Ok(())
}
