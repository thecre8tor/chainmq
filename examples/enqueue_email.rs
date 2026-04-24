use std::net::SocketAddr;

use axum::Router;
use chainmq::{
    Job, JobContext, JobOptions, Priority, Queue, QueueOptions, Result, WebUIMountConfig,
    async_trait, chainmq_dashboard_router, serde_json::json,
};
use chainmq::{JobId, RedisClient};
use serde::{Deserialize, Serialize};

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
            "[worker] (example perform) to='{}' subject='{}'",
            self.to, self.subject
        );
        ctx.set_response(json!({
            "simulated": true,
            "to": self.to,
            "subject": self.subject,
        }));
        Ok(())
    }
    fn name() -> &'static str {
        "EmailJob"
    }
    fn queue_name() -> &'static str {
        "emails"
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging to see library tracing output if any
    tracing_subscriber::fmt::try_init().ok();
    println!("[enqueue] Preparing QueueOptions and connecting to Redis...");
    let redis_url = "redis://localhost:6370".to_string();
    let options = QueueOptions {
        redis: RedisClient::Url(redis_url.clone()),
        ..Default::default()
    };
    let queue = Queue::new(options).await?;
    println!(
        "[enqueue] Connected to Redis and initialized queue '{}'.",
        EmailJob::queue_name()
    );

    let job = EmailJob {
        to: "user@example.com".into(),
        subject: "Welcome!".into(),
        body: "Thanks for signing up".into(),
    };
    println!("[enqueue] Enqueuing simple EmailJob...");
    let job_id = queue.enqueue(job).await?;
    println!("[enqueue] Enqueued EmailJob with id={}", job_id);

    let urgent = EmailJob {
        to: "user@example.com".into(),
        subject: "Urgent".into(),
        body: "Please read".into(),
    };

    let opts = JobOptions {
        delay_secs: Some(60),
        priority: Priority::Normal,
        attempts: 5,
        job_id: Some(JobId::from_string(format!(
            "CHAIN-UNIQUE-{}",
            uuid::Uuid::new_v4()
        ))),
        ..Default::default()
    };

    println!("[enqueue] Enqueuing delayed EmailJob (delay=60s, attempts=5)...");
    let job_id2 = queue.enqueue_with_options(urgent, opts).await?;
    println!(
        "[enqueue] Enqueued delayed EmailJob with id={} — it stays in Delayed until a worker promotes due jobs, then runs perform.",
        job_id2
    );

    println!("\n[enqueue] Jobs have been enqueued!");
    println!(
        "[enqueue] Tip: run a worker (see examples) so delayed jobs move to waiting after 60s and complete via perform — do not call complete_job on a delayed job from the producer unless you intend to skip execution."
    );

    let mount = WebUIMountConfig {
        ui_path: "/dashboard".to_string(),
        ..Default::default()
    };
    let dashboard = chainmq_dashboard_router(queue, mount)?;
    let app = Router::new().nest_service("/dashboard", dashboard);
    let addr = SocketAddr::from(([127, 0, 0, 1], 8085));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("[enqueue] Dashboard at http://127.0.0.1:8085/dashboard/ (Ctrl+C to stop)");
    axum::serve(listener, app).await?;

    Ok(())
}
