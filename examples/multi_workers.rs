use chainmq::{AppContext, Job, JobContext, JobRegistry, Result, WorkerBuilder, async_trait};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
struct EmailJob {
    to: String,
}

#[async_trait]
impl Job for EmailJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        println!("[emails] {}", self.to);
        Ok(())
    }
    fn name() -> &'static str {
        "EmailJob"
    }
    fn queue_name() -> &'static str {
        "emails"
    }
}

#[derive(Serialize, Deserialize)]
struct ReportJob {
    id: u32,
}

#[async_trait]
impl Job for ReportJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        println!("[reports] {}", self.id);
        Ok(())
    }
    fn name() -> &'static str {
        "ReportJob"
    }
    fn queue_name() -> &'static str {
        "reports"
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
    let mut reg_emails = JobRegistry::new();
    reg_emails.register::<EmailJob>();
    let mut reg_reports = JobRegistry::new();
    reg_reports.register::<ReportJob>();

    let redis = option_env!("REDIS_URL").unwrap_or("redis://localhost:6379");

    let mut w_emails = WorkerBuilder::new(redis, reg_emails)
        .with_app_context(app.clone())
        .with_queue_name("emails")
        .with_concurrency(3)
        .spawn()
        .await?;

    let mut w_reports = WorkerBuilder::new(redis, reg_reports)
        .with_app_context(app)
        .with_queue_name("reports")
        .with_concurrency(2)
        .spawn()
        .await?;

    w_emails.start().await?;
    w_reports.start().await?;
    println!("[multi-workers] running: emails + reports");
    tokio::signal::ctrl_c().await?;
    w_emails.stop().await;
    w_reports.stop().await;
    Ok(())
}
