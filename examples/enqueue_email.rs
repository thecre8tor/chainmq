use std::net::SocketAddr;

use axum::Router;
use chainmq::RedisClient;
use chainmq::{
    Job, JobContext, JobOptions, Priority, Queue, QueueOptions, Result, WebUIMountConfig,
    async_trait, chainmq_dashboard_router, serde_json::json,
};
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
    let queue_name = EmailJob::queue_name();
    println!(
        "[enqueue] Connected to Redis and initialized queue '{}'.",
        queue_name
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

    let response = json!({
        "status": "sent",
        "to": &urgent.to,
        "subject": &urgent.subject,
    });

    let opts = JobOptions {
        delay_secs: Some(60),
        priority: Priority::High,
        attempts: 5,
        ..Default::default()
    };

    println!("[enqueue] Enqueuing delayed/high-priority EmailJob (delay=60s, attempts=5)...");
    let job_id2 = queue.enqueue_with_options(urgent, opts).await?;
    println!(
        "[enqueue] Enqueued delayed EmailJob with id={} — done.",
        job_id2
    );

    let _ = queue
        .complete_job(&job_id2, queue_name, Some(response))
        .await?;

    println!("\n[enqueue] Jobs have been enqueued!");

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
