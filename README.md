# ChainMQ

A Redis-backed, type-safe job queue for Rust. Provides job registration and execution, delayed jobs, retries with backoff, and scalable workers.

This crate is library-first. Runnable examples demonstrate typical patterns (single worker, multiple jobs, multiple workers, delayed jobs, failure/retry).

## Features

- 🚀 Redis-Powered: Built on Redis for reliable job persistence and distribution
- 🔄 Background Jobs: Process jobs asynchronously in the background
- 🏗️ Job Registry: Simple Type-safe job registration and execution
- 🔧 Worker Management: Configurable workers with lifecycle management
- ⚡ Async/Await: Full async support throughout the system
- ⏰ Delayed jobs: Schedule jobs for future execution with atomic operations
- 🗄️ Backoff strategies: Configurable retry logic for failed jobs
- 📊 Application Context: Share application state across jobs

## Quick Start:

Add ChainMQ to your `Cargo.toml`:

```toml
[dependencies]
chainmq = "0.2.0"
actix-web = "4.0"
redis = "0.23"
tokio = { version = "1.0", features = ["full"] }
serde = { version = "1.0", features = ["derive"] }
```

## Basic Usage:

### 1. Define Your Job

```rust
use chainmq::{Job, AppContext};
use serde::{Deserialize, Serialize};
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailJob {
    pub to: String,
    pub subject: String,
    pub body: String,
}

#[async_trait]
impl Job for EmailJob {
    async fn perform(&self, ctx: &JobContext) -> chainmq::Result<()> {
        if let Some(app_ctx) = ctx.app::<AppState>() {
            let response = app_ctx
                .mail_client
                .send_email(
                    &self.sender_name,
                    &self.to,
                    &self.subject,
                    &self.html_body,
                    &self.text_body,
                    &self.category,
                )
                .await;

            if let Err(error) = response {
                println!("Unable to send OTP to user: {}", error)
            } else {
                println!("Successfully sent the email: {:#?}", response.ok())
            }
        }

        Result::Ok(())
    }

    fn name() -> &'static str {
        "EmailJob"
    }

    fn queue_name() -> &'static str {
        "emails"
    }
}
```

### 2. Set Up Application Context

```rust
use chainmq::AppContext;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub mail_client: Arc<MailClient>,
    // pub database: Arc<DatabasePool>,
    // You can also add other application services that your jobs depends on.
}

impl AppContext for AppState {
    fn clone_context(&self) -> Arc<dyn AppContext> {
        Arc::new(self.clone())
    }
}
```

### 3. Configure Workers and Web Server

```rust
use chainmq::{JobRegistry, WorkerBuilder};
use actix_web::{web::Data, App, HttpServer};
use redis::Client as RedisClient;
use tokio::sync::broadcast;

async fn setup_application() -> Result<(), anyhow::Error> {
    // Initialize Redis connection
    let redis_client = RedisClient::open("redis://127.0.0.1/")?;

    // Create application state
    let app_state = Arc::new(AppState {
        mail_client: Arc::new(MailClient::new()),
        redis_client: Arc::new(redis_client),
        // database: Arc::new(DatabasePool::new().await?),
    });

    // Set up job registry
    let mut registry = JobRegistry::new();
    registry.register::<EmailJob>();

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);

    // Start background workers
    tokio::spawn(async move {
        let mut worker = WorkerBuilder::new_with_redis_instance(redis_client.clone(), registry)
            .with_app_context(app_state.clone())
            .with_queue_name(EmailJob::queue_name())
            .spawn()
            .await
            .expect("Failed to initialize workers");

        tokio::select! {
            result = worker.start() => {
                if let Err(e) = result {
                    tracing::error!("Worker failed: {:?}", e);
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Shutting down worker...");
                worker.stop().await;
            }
        }
    });

    // Start web server
    HttpServer::new(move || {
        App::new()
            .app_data(Data::new(app_state.clone()))
            .service(your_routes)
    })
    .bind("127.0.0.1:8000")?
    .run()
    .await?;

    Ok(())
}
```

### 4. Enqueue Jobs from Your Handlers

```rust
use actix_web::{web, HttpResponse, Result};

async fn send_welcome_email(
    app_state: web::Data<AppState>
) -> Result<HttpResponse> {
    let email_job = EmailJob {
        to: "user@example.com".to_string(),
        subject: "Welcome!".to_string(),
        body: "Thank you for signing up!".to_string(),
    };

    let options = QueueOptions {
        redis_instance: Some(app_state.redis_client),
        ..Default::default()
    };

    let queue = Queue::new(options)
        .await
        .expect("Unable to initialize queue");

    // Enqueue the job
    let job_response = queue.enqueue(job).await;

    if let Err(error) = job_response {
       tracing::error!("Failed to enqueue email verification: {}", error);
    } else {
       tracing::info!("Enqueued email verification for {}", response.email);
    };

    Ok(HttpResponse::Ok().json("Welcome email queued"))
}
```

## Examples

This repo provides runnable examples. Build them all:

```bash
cargo build --examples
```

Run Redis first, then in separate terminals run workers/enqueuers.

- Single worker for emails queue:

```bash
cargo run --example worker_main
```

- Enqueue email jobs (normal + delayed/high priority):

```bash
cargo run --example enqueue_email
```

- One worker handling multiple job types on a single queue:

```bash
cargo run --example multi_jobs_single_worker
```

- Two workers on different queues (emails + reports):

```bash
cargo run --example multi_workers
```

- Failure and retry with backoff demonstration:

```bash
cargo run --example failure_retry
```

- Delayed jobs demonstration:

```bash
cargo run --example delayed_jobs
```

Notes:

- You can enqueue before or after workers start. Jobs persist in Redis until claimed.
- Ensure both worker and enqueuer use the same Redis URL and queue name.
- Some examples default to `redis://localhost:6379`. Adjust to your setup.

## Core concepts

- Job: `trait Job { async fn perform(&self, &JobContext) -> Result<()>; fn name() -> &str; fn queue_name() -> &str }`
- Queue: Persists job metadata and pushes to wait/delayed lists.
- Worker: Polls a queue, claims jobs atomically via Lua, executes via `JobRegistry`.
- Registry: Maps job type name -> executor for deserialization + dispatch.

## Service injection (AppContext)

Inject your own services (DB pools, HTTP clients, caches, etc.) via `AppContext`. The worker holds an `Arc<dyn AppContext>` and each job receives it through `JobContext`.

Define your application state:

```rust
use bullmq_rs::AppContext;
use std::sync::Arc;

#[derive(Clone)]
struct AppState {
    db: sqlx::PgPool,
    http: reqwest::Client,
}

impl AppContext for AppState {
    fn clone_context(&self) -> Arc<dyn AppContext> { Arc::new(self.clone()) }
}
```

Pass it to the worker:

```rust
let app = Arc::new(AppState { db: pool, http: reqwest::Client::new() });
let mut worker = WorkerBuilder::new_with_redis_uri("redis://localhost:6379", registry)
    .with_app_context(app)
    .with_queue_name("default")
    .spawn()
    .await?;
```

Use it inside jobs via the helper `ctx.app::<T>()` (preferred) or explicit downcast:

```rust
#[async_trait]
impl Job for MyJob {
    async fn perform(&self, ctx: &JobContext) -> Result<()> {
        // Preferred typed helper
        if let Some(app) = ctx.app::<AppState>() {
            let row = sqlx::query!("select 1 as one").fetch_one(&app.db).await?;
            let _ = app.http.get("https://example.com").send().await?;
            println!("db one = {}", row.one);
        }

        // Or manual downcast if needed
        // if let Some(app) = ctx.app_context.as_ref().as_any().downcast_ref::<AppState>() { /* ... */ }
        Ok(())
    }
    fn name() -> &'static str { "MyJob" }
}
```

## Internals (high level)

- Lua scripts in `src/lua` ensure atomic operations:
  - `move_delayed.lua`: moves due jobs from delayed zset to wait list
  - `claim_job.lua`: pops from wait -> adds to active, updates job state
- Redis keys (default prefix `rbq`):
  - `rbq:queue:{name}:wait`, `:active`, `:delayed`, `:failed`
  - `rbq:job:{id}` stores serialized job metadata

## Troubleshooting

- Perform not running:
  - Ensure worker `.with_queue_name(...)` matches `Job::queue_name()`
  - Same Redis URL for worker/enqueuer
  - Check `LRANGE rbq:queue:{queue}:wait 0 -1` in `redis-cli`
- Lua invocation error (arguments must be strings/integers):
  - Fixed in the crate: `move_delayed.lua` is called with `ARGV` (queue_name, now)
- No jobs claimed:
  - Verify jobs enqueued to the same queue and not delayed into the future
- Connection issues:
  - Verify Redis URL and server is running

## Development

- Build: `cargo build`
- Test: `cargo test`

## License

MIT

## Acknowledgements

Inspired by existing Redis-backed queues and workers; built for ergonomic, type-safe Rust use.
