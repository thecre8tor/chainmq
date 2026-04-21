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

Open your browser and navigate to the UI, for example `http://127.0.0.1:8080/dashboard` (adjust host, port, and path to match your `WebUIConfig`). Unless you set `WebUIConfig { auth: None, .. }`, you will be prompted to sign in; the built-in default username and password are **`ChainMQ`** / **`ChainMQ`** until you override `WebUIConfig.auth`. The dashboard loads data over HTTP paths under `{ui_path}/api`, but those handlers are **not** a supported public REST API: use the web UI, not curl or other HTTP clients.

## Job execution logs

The **Logs** tab on a job reads lines stored in **Redis** (same instance and key prefix as the queue), loaded by the dashboard over its internal JSON routes. They are **not** captured from `println!` or raw stdout: with concurrent jobs in one process, stdout is global and cannot be attributed per job. Use **`tracing`** (`info!`, `debug!`, etc.) inside your job’s `perform` while the worker has entered the `job_execution` span (the library does this around your handler).

**Default:** when you start a `Worker` and the process has **no** global `tracing` subscriber yet, ChainMQ installs one: `EnvFilter` (from `RUST_LOG`, else `info`), stdout formatting, and **`job_logs_layer`** for that worker’s queue. No extra setup is required (see [`examples/worker_main.rs`](./examples/worker_main.rs)).

**Custom subscriber:** if you call `tracing_subscriber::…::init()` (or equivalent) **before** `WorkerBuilder::spawn`, you must add **`chainmq::job_logs_layer`** yourself and pass the same **`Arc<Queue>`** via **`WorkerBuilder::with_shared_queue`**, so the layer and the worker share one queue handle.

Optional: cap retention with **`QueueOptions::job_logs_max_lines`** (default `500`). Logs for a job are removed when the job row is deleted.

## Configuration Options

### WebUIConfig

**Bind address** (`bind_host`), **port**, and **HTTP base path** (`ui_path`) are configurable. Static assets are always loaded from **`./ui`** relative to the process current working directory (see [UI files](#ui-files)).

```rust
pub struct WebUIConfig {
    pub bind_host: String,
    pub port: u16,
    pub ui_path: String,
    /// When `Some`, the dashboard requires login (signed HttpOnly session cookie). Default: `Some(WebUIAuth::default())`.
    pub auth: Option<WebUIAuth>,
    /// 64-byte signing key for session cookies; if `None` while `auth` is set, a fixed **dev-only** key is used.
    pub session_secret: Option<[u8; 64]>,
    /// Session cookie `Secure` flag (use `true` behind HTTPS).
    pub cookie_secure: bool,
}

pub struct WebUIAuth {
    pub username: String,
    pub password: String,
}
```

Defaults for `WebUIAuth` are username **`ChainMQ`** and password **`ChainMQ`** (for local use only; override in production).

### Examples

#### Default Configuration (Root Path)

```rust
let config = WebUIConfig::default();
// UI: http://127.0.0.1:8085/  (default port is 8085)
// Login is enabled by default (username / password: ChainMQ / ChainMQ) until you change `auth`.
```

#### Custom Dashboard Path

```rust
let config = WebUIConfig {
    port: 8080,
    ui_path: "/dashboard".to_string(),
    ..Default::default()
};
// UI: http://127.0.0.1:8080/dashboard
```

#### Custom Port and Path

```rust
let config = WebUIConfig {
    port: 3000,
    ui_path: "/admin/queues".to_string(),
    ..Default::default()
};
// UI: http://127.0.0.1:3000/admin/queues
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

#### Dashboard login (default on)

`WebUIConfig::default()` enables session login with `WebUIAuth::default()` (**`ChainMQ` / `ChainMQ`**). Set your own operator credentials in Rust:

```rust
use chainmq::{WebUIAuth, WebUIConfig};

let config = WebUIConfig {
    auth: Some(WebUIAuth {
        username: "operator".into(),
        password: std::env::var("DASHBOARD_PASSWORD").unwrap(),
    }),
    // Use a random 64-byte key in production, e.g. read from env or a secrets manager:
    // session_secret: Some(*b"0123456789012345678901234567890123456789012345678901234567890123"),
    cookie_secure: true, // when serving the UI over HTTPS
    ..Default::default()
};
```

To **turn off** the login screen (only for trusted local use), set `auth: None`:

```rust
let config = WebUIConfig {
    auth: None,
    ..Default::default()
};
```

## UI files

The dashboard expects these files under **`./ui`** (relative to the working directory when the process starts):

- `index.html` — main HTML structure
- `app.js` — frontend logic
- `styles.css` — styling

Copy or symlink the [`ui/`](./ui/) directory from this repository next to your binary, or run from the project root during development. The SPA resolves its data base path from `ui_path` (paths under `{ui_path}/api/...`).

## HTTP JSON and the dashboard

The UI issues `fetch` calls to `{ui_path}/api/...` with **credentials** (session cookie) when auth is enabled. Those routes exist only to support the bundled dashboard: they require a **same-origin** (or same-site) browser request (`Sec-Fetch-Site`), so command-line tools and generic HTTP clients receive **403 Forbidden**. They are not documented as a stable public API; operate the queue through the UI (or through your Rust `Queue` / workers in application code).

## Running in Production

For production use, consider:

1. **Reverse Proxy**: Use nginx or similar to handle SSL/TLS
2. **Authentication**: Set `WebUIConfig.auth` with strong credentials, `session_secret: Some([u8; 64])` from a CSPRNG, and `cookie_secure: true` when serving over HTTPS. The built-in login is for an **internal operator dashboard**, not multi-tenant identity.
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

### Dashboard data not loading

- Open the UI in a normal browser tab (same origin as the server). Direct `curl` or scripts against `/api/...` are intentionally rejected.
- Check the browser console and network tab for failed requests.
- Verify `ui_path` matches how you open the app (including trailing slashes vs none).
- Ensure Redis is reachable and the queue is initialized.

### Port already in use

- Change the `port` in `WebUIConfig`
- Or stop the process using that port
