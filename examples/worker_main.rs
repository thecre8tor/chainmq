use chainmq::{AppContext, Job, JobContext, JobRegistry, Result, WorkerBuilder, async_trait};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
struct EmailJob {
    to: String,
    subject: String,
    body: String,
}

#[async_trait]
impl Job for EmailJob {
    async fn perform(&self, ctx: &JobContext) -> Result<()> {
        println!(
            "[worker] Executing EmailJob → to='{}' subject='{}'",
            self.to, self.subject
        );
        if let Some(app) = ctx.app::<AppState>() {
            app.email_service
                .send(&self.to, &self.subject, &self.body)
                .await?;
        }
        println!("[worker] EmailJob completed for to='{}'", self.to);
        Ok(())
    }

    fn name() -> &'static str {
        "EmailJob"
    }
    fn queue_name() -> &'static str {
        "emails"
    }
}

#[derive(Clone, Default)]
struct AppState {
    email_service: EmailService,
}

impl AppContext for AppState {
    fn clone_context(&self) -> Arc<dyn AppContext> {
        Arc::new(self.clone())
    }
}

#[derive(Clone, Default)]
struct EmailService;

impl EmailService {
    async fn send(&self, _to: &str, _subject: &str, _body: &str) -> Result<()> {
        println!("Sent email to {}: {}", _to, _subject);
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging to see library tracing output
    tracing_subscriber::fmt::try_init().ok();
    println!("[boot] Initializing AppState and dependencies...");
    let app_state = Arc::new(AppState::default());
    println!("[boot] AppState initialized.");

    println!("[boot] Creating JobRegistry and registering EmailJob...");
    let mut registry = JobRegistry::new();
    registry.register::<EmailJob>();
    println!(
        "[boot] Registered job: {} on queue '{}'",
        EmailJob::name(),
        EmailJob::queue_name()
    );

    let redis_url = "redis://localhost:6379";
    let concurrency = 5usize;
    println!(
        "[boot] Spawning worker → redis='{}' concurrency={} queue='{}'",
        redis_url,
        concurrency,
        EmailJob::queue_name()
    );
    let mut worker = WorkerBuilder::new(redis_url, registry)
        .with_app_context(app_state)
        .with_concurrency(concurrency)
        .with_queue_name(EmailJob::queue_name())
        .spawn()
        .await?;

    println!("[worker] Starting worker event loops...");
    worker.start().await?;
    println!("[worker] Worker started. Press Ctrl+C to stop.");

    tokio::signal::ctrl_c().await?;
    println!("[worker] Shutdown signal received. Stopping worker...");
    worker.stop().await;
    println!("[worker] Worker stopped. Goodbye.");

    Ok(())
}
