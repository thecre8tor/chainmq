use chainmq::{
    AppContext, Job, JobContext, JobRegistry, Result, WorkerBuilder, async_trait, serde_json::json,
};
use redis::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

#[derive(Serialize, Deserialize)]
struct EmailJob {
    to: String,
    subject: String,
    body: String,
}

#[async_trait]
impl Job for EmailJob {
    async fn perform(&self, ctx: &JobContext) -> Result<()> {
        // `tracing` events here are stored in Redis and shown in the web UI Job → Logs tab
        // (the worker installs `job_logs_layer` if you have not set a global subscriber).
        // `println!` is not captured.
        info!(
            to = %self.to,
            subject = %self.subject,
            "executing EmailJob"
        );
        if let Some(app) = ctx.app::<AppState>() {
            app.email_service
                .send(&self.to, &self.subject, &self.body)
                .await?;
        }
        ctx.set_response(json!({
            "status": "sent",
            "to": self.to,
            "subject": self.subject,
            "message_id": format!("ex-{}", ctx.job_id),
        }));
        info!(to = %self.to, "EmailJob completed");
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
    async fn send(&self, to: &str, subject: &str, _body: &str) -> Result<()> {
        info!(%to, %subject, "email send simulated");
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("[boot] Initializing AppState and dependencies...");
    let app_state = Arc::new(AppState::default());

    println!("[boot] Creating JobRegistry and registering EmailJob...");
    let mut registry = JobRegistry::new();
    registry.register::<EmailJob>();

    let redis_url = "redis://localhost:6370";
    let concurrency = 5usize;
    println!(
        "[boot] Spawning worker → redis='{}' concurrency={} queue='{}'",
        redis_url,
        concurrency,
        EmailJob::queue_name()
    );

    let client = Client::open(redis_url)?;
    let mut worker = WorkerBuilder::new_with_redis_instance(&client, registry)
        .with_app_context(app_state)
        .with_concurrency(concurrency)
        .with_queue_name(EmailJob::queue_name())
        .spawn()
        .await?;

    println!("[worker] Starting worker event loops...");
    worker.start().await?;

    Ok(())
}
