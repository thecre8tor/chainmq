# ChainMQ Web UI

The ChainMQ Web UI provides a modern, BullMQ-style dashboard for monitoring and managing your job queues. Run **one** UI server with a `Queue` configured for your Redis instance and `key_prefix`; it lists and manages **every** logical queue name (`Job::queue_name()`) in that namespace.

## Features

- 🎨 Modern, classy UI with light/dark mode
- 📊 Real-time queue statistics
- 🔍 Job search and filtering
- 📄 Pagination for large job lists
- ⚡ Queue actions (clean, recover stalled, process delayed)
- 🔄 Auto-refresh every 3 seconds
- 📱 Responsive design
- 📝 Per-job execution logs (when workers use `job_logs_layer`; see [Job execution logs](#job-execution-logs))

## Quick Start

### 1. Enable the web-ui feature

The `web-ui` feature is **enabled by default** on `chainmq` 0.2.0. You only need to set it explicitly when you disabled default features:

```toml
[dependencies]
chainmq = { version = "0.2.0", features = ["web-ui"], default-features = false }
```

Otherwise a plain dependency on `chainmq = "0.2.0"` is enough for `start_web_ui` / `start_web_ui_simple`.

### 2. Start the UI server

```rust
use chainmq::{Queue, QueueOptions, start_web_ui, WebUIConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create your queue
    let queue = Queue::new(QueueOptions {
        redis_url: "redis://localhost:6379".to_string(),
        ..Default::default()
    }).await?;

    let ui_config = WebUIConfig {
        port: 8080,
        ui_path: "/dashboard".to_string(),
        ..Default::default()
    };

    // Start the server
    start_web_ui(queue, ui_config).await?.await?;

    Ok(())
}
```

### 3. Access the Dashboard

Open your browser and navigate to:
- **UI**: `http://127.0.0.1:8080/dashboard`
- **API**: `http://127.0.0.1:8080/dashboard/api`

## Job execution logs

The **Logs** tab on a job calls `GET …/api/jobs/{job_id}/logs` and reads lines stored in **Redis** (same instance and key prefix as the queue). They are **not** captured from `println!` or raw stdout: with concurrent jobs in one process, stdout is global and cannot be attributed per job. Use **`tracing`** (`info!`, `debug!`, etc.) inside your job’s `perform` while the worker has entered the `job_execution` span (the library does this around your handler).

**Default:** when you start a `Worker` and the process has **no** global `tracing` subscriber yet, ChainMQ installs one: `EnvFilter` (from `RUST_LOG`, else `info`), stdout formatting, and **`job_logs_layer`** for that worker’s queue. No extra setup is required (see [`examples/worker_main.rs`](./examples/worker_main.rs)).

**Custom subscriber:** if you call `tracing_subscriber::…::init()` (or equivalent) **before** `WorkerBuilder::spawn`, you must add **`chainmq::job_logs_layer`** yourself and pass the same **`Arc<Queue>`** via **`WorkerBuilder::with_shared_queue`**, so the layer and the worker share one queue handle.

Optional: cap retention with **`QueueOptions::job_logs_max_lines`** (default `500`). Logs for a job are removed when the job row is deleted.

## Configuration Options

### WebUIConfig

**Bind address** (`bind_host`), **port**, and **HTTP base path** (`ui_path`) are configurable. Static assets are always loaded from **`./ui`** relative to the process current working directory (see [UI files](#ui-files)).

```rust
pub struct WebUIConfig {
    /// Host or IP to bind (default: "127.0.0.1"). Use "0.0.0.0" for all IPv4 interfaces
    /// when the process must accept remote connections (use a firewall or proxy in production).
    pub bind_host: String,

    /// Port to bind the server to (default: 8080)
    pub port: u16,

    /// Base path for the UI (default: "/")
    /// Examples: "/dashboard", "/admin/queues", "/monitoring"
    pub ui_path: String,
}
```

### Examples

#### Default Configuration (Root Path)

```rust
let config = WebUIConfig::default();
// UI: http://127.0.0.1:8080/
// API: http://127.0.0.1:8080/api
```

#### Custom Dashboard Path

```rust
let config = WebUIConfig {
    port: 8080,
    ui_path: "/dashboard".to_string(),
    ..Default::default()
};
// UI: http://127.0.0.1:8080/dashboard
// API: http://127.0.0.1:8080/dashboard/api
```

#### Custom Port and Path

```rust
let config = WebUIConfig {
    port: 3000,
    ui_path: "/admin/queues".to_string(),
    ..Default::default()
};
// UI: http://127.0.0.1:3000/admin/queues
// API: http://127.0.0.1:3000/admin/queues/api
```

#### Listen on All Interfaces (e.g. Containers or Direct LAN Access)

```rust
let config = WebUIConfig {
    bind_host: "0.0.0.0".to_string(),
    port: 8080,
    ..Default::default()
};
// Prefer a reverse proxy or firewall when bind_host is not loopback.
```

## UI files

The dashboard expects these files under **`./ui`** (relative to the working directory when the process starts):

- `index.html` — main HTML structure
- `app.js` — frontend logic
- `styles.css` — styling

Copy or symlink the [`ui/`](./ui/) directory from this repository next to your binary, or run from the project root during development. The API base path follows `ui_path` (e.g. `{ui_path}/api`).

## API Endpoints

All API endpoints are prefixed with `{ui_path}/api`:

- `GET /api/queues` - List all queues
- `GET /api/queues/{queue_name}/stats` - Get queue statistics
- `GET /api/queues/{queue_name}/jobs/{state}` - List jobs by state
- `GET /api/jobs/{job_id}` - Get job details
- `GET /api/jobs/{job_id}/logs?limit=200` - Job execution log lines (from `job_logs_layer`; optional `limit`, max 500)
- `POST /api/jobs/{job_id}/retry` - Retry a failed job
- `DELETE /api/jobs/{job_id}/delete` - Delete a job
- `POST /api/queues/clean` - Clean jobs by state
- `POST /api/queues/{queue_name}/recover-stalled` - Recover stalled jobs
- `POST /api/queues/{queue_name}/process-delayed` - Process delayed jobs

## Running in Production

For production use, consider:

1. **Reverse Proxy**: Use nginx or similar to handle SSL/TLS
2. **Authentication**: Add authentication middleware before starting the UI
3. **Static assets**: Ensure a `./ui` directory (with the three files above) exists relative to the process working directory—e.g. copy or symlink the `ui` folder beside your binary and set the service `WorkingDirectory` accordingly

Example with environment variables for **port** and **path** only:

```rust
let config = WebUIConfig {
    port: std::env::var("UI_PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()
        .unwrap_or(8080),
    ui_path: std::env::var("UI_PATH").unwrap_or_else(|_| "/".to_string()),
};
```

## Troubleshooting

### UI not loading

- Ensure `./ui` exists from the process working directory and contains `index.html`, `app.js`, and `styles.css`
- Check browser console for errors

### API calls failing

- Verify the API path matches your `ui_path` configuration
- Check that the queue is properly initialized
- Ensure Redis is accessible

### Port already in use

- Change the `port` in `WebUIConfig`
- Or stop the process using that port
