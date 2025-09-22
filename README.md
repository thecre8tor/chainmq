# rustque

A Redis-backed, type-safe job queue for Rust. Provides job registration and execution, delayed jobs, retries with backoff, and scalable workers.

This crate is library-first. Runnable examples demonstrate typical patterns (single worker, multiple jobs, multiple workers, delayed jobs, failure/retry).

## Features

- Type-safe jobs with `serde` payloads
- Redis-backed persistence (wait/active/delayed/failed)
- Delayed jobs via Lua atomics
- Retries with pluggable backoff strategies
- Scalable workers with configurable concurrency
- Simple registration and execution model

## Requirements

- Rust (stable)
- Redis server (local or remote)

Start Redis locally (examples default to localhost):

```bash
redis-server
```

## Install (use in another crate)

In your app's `Cargo.toml`:

```toml
[dependencies]
rustque = { path = "/absolute/path/to/rustque" }
```

## Quick start (library usage)

Define a job type and run a worker in your application:

```rust
use rustque::{async_trait, Job, JobContext, JobRegistry, WorkerBuilder, Result, AppContext};
use serde::{Serialize, Deserialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize)]
struct Hello;

#[async_trait]
impl Job for Hello {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        println!("hello from job");
        Ok(())
    }
    fn name() -> &'static str { "Hello" }
    fn queue_name() -> &'static str { "default" }
}

#[derive(Clone, Default)]
struct AppState;
impl AppContext for AppState { fn clone_context(&self) -> Arc<dyn AppContext> { Arc::new(self.clone()) } }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app = Arc::new(AppState::default());
    let mut registry = JobRegistry::new();
    registry.register::<Hello>();

    let mut worker = WorkerBuilder::new("redis://localhost:6379", registry)
        .with_app_context(app)
        .with_queue_name("default")
        .with_concurrency(5)
        .spawn()
        .await?;

    worker.start().await?;
    // enqueue from somewhere else in your app:
    // let queue = rustque::Queue::new(rustque::QueueOptions::default()).await?;
    // queue.enqueue(Hello).await?;

    tokio::signal::ctrl_c().await?;
    worker.stop().await;
    Ok(())
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
- Some examples default to `redis://localhost:6370`. Adjust to your setup.

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
let mut worker = WorkerBuilder::new("redis://localhost:6379", registry)
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

MIT (or your chosen license)

## Acknowledgements

Inspired by existing Redis-backed queues and workers; built for ergonomic, type-safe Rust use.
