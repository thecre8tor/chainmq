// examples/multi_jobs_with_ui.rs
// Example showing how to integrate UI with multiple job types

use chainmq::{
    Job, JobContext, JobOptions, Priority, Queue, QueueOptions, RedisClient, Result, async_trait,
    start_web_ui_simple,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct EmailJob {
    to: String,
    subject: String,
}

#[derive(Serialize, Deserialize)]
struct ReportJob {
    report_id: u32,
    format: String,
}

#[derive(Serialize, Deserialize)]
struct NotificationJob {
    user_id: u32,
    message: String,
}

#[async_trait]
impl Job for EmailJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        println!(
            "[EmailJob] Sending email to={} subject={}",
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

#[async_trait]
impl Job for ReportJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        println!(
            "[ReportJob] Generating report_id={} format={}",
            self.report_id, self.format
        );
        Ok(())
    }
    fn name() -> &'static str {
        "ReportJob"
    }
    fn queue_name() -> &'static str {
        "reports"
    }
}

#[async_trait]
impl Job for NotificationJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        println!(
            "[NotificationJob] Notifying user_id={} message={}",
            self.user_id, self.message
        );
        Ok(())
    }
    fn name() -> &'static str {
        "NotificationJob"
    }
    fn queue_name() -> &'static str {
        "notifications"
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::try_init().ok();
    println!("[multi-jobs] Connecting to Redis...");

    let redis_url = "redis://localhost:6370".to_string();
    let queue = Queue::new(QueueOptions {
        redis: RedisClient::Url(redis_url.clone()),
        ..Default::default()
    })
    .await?;

    // Enqueue jobs to different queues
    println!("\n[multi-jobs] Enqueuing jobs to different queues...");

    // Email queue
    let email = EmailJob {
        to: "user@example.com".to_string(),
        subject: "Welcome!".to_string(),
    };
    let email_id = queue.enqueue(email).await?;
    println!(
        "[multi-jobs] Enqueued EmailJob (id={}) to 'emails' queue",
        email_id
    );

    // Report queue
    let report = ReportJob {
        report_id: 123,
        format: "pdf".to_string(),
    };
    let report_id = queue.enqueue(report).await?;
    println!(
        "[multi-jobs] Enqueued ReportJob (id={}) to 'reports' queue",
        report_id
    );

    // Notification queue with options
    let notification = NotificationJob {
        user_id: 456,
        message: "You have a new message".to_string(),
    };
    let opts = JobOptions {
        priority: Priority::High,
        delay_secs: Some(30),
        ..Default::default()
    };
    let notif_id = queue.enqueue_with_options(notification, opts).await?;
    println!(
        "[multi-jobs] Enqueued NotificationJob (id={}) to 'notifications' queue",
        notif_id
    );

    println!("\n[multi-jobs] All jobs enqueued!");
    println!("[multi-jobs] The UI will automatically discover and display all queues");

    // Start the web UI - it automatically discovers ALL queues from Redis!
    // You only need ONE call, regardless of how many job types/queues you have
    let ui_queue = Queue::new(QueueOptions {
        redis: RedisClient::Url(redis_url),
        ..Default::default()
    })
    .await?;
    start_web_ui_simple(ui_queue).await?;

    Ok(())
}
