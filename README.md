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
                .send_email(&self.to, &self.subject, &self.body)
                .await;

            match response {
                Ok(result) => println!("Email sent successfully: {:#?}", result),
                Err(error) => println!("Failed to send email: {}", error),
            }
        }

        Ok(())
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
    pub redis_client: Arc<redis::Client>,
}

impl AppContext for AppState {
    fn clone_context(&self) -> Arc<dyn AppContext> {
        Arc::new(self.clone())
    }
}
```

### 3. Configure Workers and Your Preffered Web Server

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
        redis_client: Arc::new(redis_client.clone()),
    });

    // Set up job registry
    let mut registry = JobRegistry::new();
    registry.register::<EmailJob>();


    // Start background workers
    tokio::spawn(async move {
        let mut worker = WorkerBuilder::new_with_redis_instance(redis_client.clone(), registry)
            .with_app_context(app_state.clone())
            .with_queue_name(EmailJob::queue_name())
            .spawn()
            .await
            .expect("Failed to initialize workers");

        let _ = worker.start().await;
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

### 4. Enqueue Jobs from Anywhere

```rust
use chainmq::{Queue, QueueOptions};

async fn enqueue_email_job(app_state: &AppState) -> chainmq::Result<()> {
    let email_job = EmailJob {
        to: "user@example.com".to_string(),
        subject: "Welcome!".to_string(),
        body: "Thank you for signing up!".to_string(),
    };

    let options = QueueOptions {
        redis_instance: Some(app_state.redis_client.clone()),
        ..Default::default()
    };

    let queue = Queue::new(options).await?;

    // Enqueue the job
    match queue.enqueue(email_job).await {
        Ok(_) => println!("Email job enqueued successfully"),
        Err(error) => eprintln!("Failed to enqueue email job: {}", error),
    }

    Ok(())
}
```

## Examples

This repo provides runnable examples. Build them all:

```bash
cargo build --examples
```

Run Redis first, then in separate terminals run workers/enqueuers:

```bash
# Single worker for emails queue
cargo run --example worker_main

# Enqueue email jobs (normal + delayed/high priority)
cargo run --example enqueue_email

# One worker handling multiple job types on a single queue
cargo run --example multi_jobs_single_worker

# Two workers on different queues (emails + reports)
cargo run --example multi_workers

# Failure and retry with backoff demonstration
cargo run --example failure_retry

# Delayed jobs demonstration
cargo run --example delayed_jobs
```

**Notes:**

- You can enqueue before or after workers start. Jobs persist in Redis until claimed.
- Ensure both worker and enqueuer use the same Redis URL and queue name.
- Some examples default to `redis://localhost:6379`. Adjust to your setup.

## Core Concepts

- **Job**: Defines work to be done. Implements `trait Job { async fn perform(&self, &JobContext) -> Result<()>; fn name() -> &str; fn queue_name() -> &str }`
- **Queue**: Persists job metadata and manages wait/delayed/active/failed job lists in Redis
- **Worker**: Polls queues, claims jobs atomically via Lua scripts, executes jobs via JobRegistry
- **Registry**: Maps job type names to executors for deserialization and dispatch
- **JobContext**: Provides access to application state and job metadata during execution

## Configuration

### Redis Configuration

ChainMQ works with any Redis instance:

```rust
// Local Redis
let redis_client = redis::Client::open("redis://127.0.0.1:6379/")?;

// Redis with authentication
let redis_client = redis::Client::open("redis://:password@127.0.0.1:6379/")?;

// Redis with database selection
let redis_client = redis::Client::open("redis://127.0.0.1:6379/1")?;
```

### Worker Configuration

```rust
// Using Redis instance
let worker = WorkerBuilder::new_with_redis_instance(redis_client, registry)
    .with_app_context(app_state)
    .with_queue_name("priority_queue")
    .with_concurrency(10)                           // Number of concurrent jobs
    .with_poll_interval(Duration::from_secs(5))     // How often to check for jobs
    .spawn()
    .await?;

// Using Redis URI
let worker = WorkerBuilder::new_with_redis_uri("redis://127.0.0.1:6379/", registry)
    .with_app_context(app_state)
    .with_queue_name("background_tasks")
    .with_concurrency(5)
    .spawn()
    .await?;
```

### Queue Configuration

```rust
let options = QueueOptions {
    name: "default".to_string(),
    redis_url: "redis://127.0.0.1:6379".to_string(),
    redis_instance: None,
    key_prefix: "rbq".to_string(),
    default_concurrency: 10,
    max_stalled_interval: 30000, // 30 seconds
};

let queue = Queue::new(options).await?;
```

### Job Configuration

```rust
let job = EmailJob {
    to: "user@example.com".into(),
    subject: "Urgent".into(),
    body: "Please read".into(),
};

let opts = JobOptions {
    delay_secs: Some(60),
    priority: Priority::High,
    attempts: 5,
    backoff: BackoffStrategy::Exponential { base: 2, cap: 10 },
    timeout_secs: Some(60),
    rate_limit_key: Some("i_rater".to_string()),
};

let job_id = queue.enqueue_with_options(job, opts).await?;
```

## Advanced Usage

### Service Injection with AppContext

Inject your own services (database pools, HTTP clients, caches, etc.) via `AppContext`. The worker holds an `Arc<dyn AppContext>` and each job receives it through `JobContext`.

```rust
use chainmq::AppContext;
use std::sync::Arc;

#[derive(Clone)]
struct AppState {
    pub database: sqlx::PgPool,
    pub http_client: reqwest::Client,
    pub cache: Arc<RedisCache>,
    pub mail_client: Arc<MailClient>,
}

impl AppContext for AppState {
    fn clone_context(&self) -> Arc<dyn AppContext> {
        Arc::new(self.clone())
    }
}
```

Use it inside jobs via the helper `ctx.app::<T>()`:

```rust
#[async_trait]
impl Job for DatabaseJob {
    async fn perform(&self, ctx: &JobContext) -> chainmq::Result<()> {
        if let Some(app) = ctx.app::<AppState>() {
            // Use database
            let user = sqlx::query_as!(User, "SELECT * FROM users WHERE id = $1", self.user_id)
                .fetch_one(&app.database)
                .await?;

            // Use HTTP client
            let response = app.http_client
                .get("https://api.example.com/data")
                .send()
                .await?;

            // Use cache
            app.cache.set(&format!("user:{}", user.id), &user).await?;
        }

        Ok(())
    }

    fn name() -> &'static str { "DatabaseJob" }
    fn queue_name() -> &'static str { "database" }
}
```

### Multiple Job Types

Register multiple job types in a single registry:

```rust
let mut registry = JobRegistry::new();
registry.register::<EmailJob>();
registry.register::<ImageProcessingJob>();
registry.register::<ReportGenerationJob>();
registry.register::<CleanupJob>();

// Single worker can handle all job types
let worker = WorkerBuilder::new_with_redis_instance(redis_client, registry)
    .with_queue_name("mixed_jobs")
    .spawn()
    .await?;
```

### Delayed Jobs

Schedule jobs for future execution:

```rust
use chainmq::JobOptions;
use std::time::Duration;

let delayed_job = EmailJob {
    to: "user@example.com".to_string(),
    subject: "Reminder".to_string(),
    body: "Don't forget about your appointment!".to_string(),
};

let options = JobOptions {
    delay_secs: Some(3600), // 1 hour delay
    ..Default::default()
};

queue.enqueue_with_options(delayed_job, options).await?;
```

### Error Handling and Retries

Jobs that fail are automatically retried with configurable backoff:

```rust
#[async_trait]
impl Job for RiskyJob {
    async fn perform(&self, ctx: &JobContext) -> chainmq::Result<()> {
        // This job might fail and will be retried
        if random::<f32>() < 0.3 {
            return Err("Random failure".into());
        }

        println!("Job succeeded!");
        Ok(())
    }

    fn name() -> &'static str { "RiskyJob" }
    fn queue_name() -> &'static str { "risky" }
}
```

## Service injection (AppContext)

Inject your own services (DB pools, HTTP clients, caches, etc.) via `AppContext`. The worker holds an `Arc<dyn AppContext>` and each job receives it through `JobContext`.

Define your application state:

```rust
use chainmq::AppContext;
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

ChainMQ uses Lua scripts to ensure atomic operations:

- **`move_delayed.lua`**: Moves due jobs from delayed sorted set to wait list
- **`claim_job.lua`**: Atomically pops from wait list and adds to active list

Redis keys use a configurable prefix (default `rbq`):

- `rbq:queue:{name}:wait` - Jobs waiting to be processed
- `rbq:queue:{name}:active` - Jobs currently being processed
- `rbq:queue:{name}:delayed` - Jobs scheduled for future execution
- `rbq:queue:{name}:failed` - Jobs that have failed processing
- `rbq:job:{id}` - Individual job metadata and payload

## Troubleshooting

**Jobs not being processed:**

- Ensure worker `.with_queue_name()` matches `Job::queue_name()`
- Verify same Redis URL for both worker and enqueuer
- Check jobs are enqueued: `redis-cli LRANGE rbq:queue:{queue}:wait 0 -1`

**Connection issues:**

- Verify Redis server is running and accessible
- Check Redis URL format and credentials
- Test connection with `redis-cli ping`

**Jobs failing silently:**

- Check Redis logs and failed job queue: `LRANGE rbq:queue:{queue}:failed 0 -1`
- Add logging/tracing to your job implementations
- Ensure job payload can be properly serialized/deserialized

**Performance issues:**

- Increase worker concurrency with `.with_concurrency(n)`
- Reduce poll interval with `.with_poll_interval(duration)`
- Monitor Redis memory usage and job queue lengths

## Development

```bash
# Build the library
cargo build

# Run examples (requires Redis)
cargo run --example worker_main
```

## License

MIT

## Acknowledgements

Inspired by existing Redis-backed job queues; built for ergonomic, type-safe Rust applications.
