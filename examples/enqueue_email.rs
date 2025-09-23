use chainmq::{Job, JobContext, JobOptions, Priority, Queue, QueueOptions, Result, async_trait};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct EmailJob {
    to: String,
    subject: String,
    body: String,
}

#[async_trait]
impl Job for EmailJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        println!(
            "[worker] (example perform) to='{}' subject='{}'",
            self.to, self.subject
        );
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
    let options = QueueOptions {
        redis_url: "redis://localhost:6379".to_string(),
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

    Ok(())
}
