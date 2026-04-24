# ChainMQ Web UI

The ChainMQ Web UI provides a modern, BullMQ-style dashboard for monitoring and managing your job queues. Use **one** [`Queue`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html) configured for your Redis instance and `key_prefix`; it lists and manages **every** logical queue name (`Job::queue_name()`) in that namespace.

The library **does not start an HTTP server**. You choose the host, port, and TLS on your Axum or Actix app, then **mount** the dashboard routes at your chosen base path.

## Features

- 🎨 Modern, classy UI with light/dark mode
- 📊 Real-time queue statistics
- 🔍 Job search and filtering
- 📄 Pagination for large job lists
- ⚡ Queue actions (clean, recover stalled, process delayed)
- 🔄 Auto-refresh every 3 seconds
- 📱 Responsive design
- 📝 Per-job execution logs (when workers use `job_logs_layer`; see [Job execution logs](#job-execution-logs))

## Responsive layout

The UI scales from **desktop** (persistent sidebar, wide tables, multi-column job detail) to **mobile** (top chrome with menu drawer, stacked stat cards, single-column job detail and logs). Source files for the dashboard live under [`ui/`](./ui/) in this repository; at **compile time** they are embedded into the `chainmq` library binary (see [UI files](#ui-files)).

|                                        Desktop — queue                                         |                                      Desktop — job detail                                       |
| :--------------------------------------------------------------------------------------------: | :---------------------------------------------------------------------------------------------: |
| ![ChainMQ dashboard: queue and jobs on a wide screen](docs/images/dashboard/desktop-queue.png) | ![ChainMQ dashboard: job detail on a wide screen](docs/images/dashboard/desktop-job-detail.png) |

|                                       Mobile — queue                                        |                                       Mobile — job detail                                        |
| :-----------------------------------------------------------------------------------------: | :----------------------------------------------------------------------------------------------: |
| ![ChainMQ dashboard: queue view on a narrow screen](docs/images/dashboard/mobile-queue.png) | ![ChainMQ dashboard: job detail on a narrow screen](docs/images/dashboard/mobile-job-detail.png) |

## Quick start

### 1. Cargo features

- **`web-ui`** (default): alias for **`web-ui-axum`** — nestable **Axum** [`Router`](https://docs.rs/axum/latest/axum/struct.Router.html) via [`chainmq_dashboard_router`](https://docs.rs/chainmq/latest/chainmq/fn.chainmq_dashboard_router.html).
- **`web-ui-axum`**: Axum + cookie-backed sessions (same idea as before: dashboard-only JSON routes).
- **`web-ui-actix`**: optional **Actix** integration via [`configure_chainmq_web_ui`](https://docs.rs/chainmq/latest/chainmq/fn.configure_chainmq_web_ui.html) (no `HttpServer` inside `chainmq`).

Smaller builds without any dashboard:

```toml
[dependencies]
chainmq = { version = "1.2.1", default-features = false }
```

Actix-only consumers:

```toml
chainmq = { version = "1.2.1", default-features = false, features = ["web-ui-actix"] }
```

### 2. Axum: mount the router

Redis is selected on [`QueueOptions`](https://docs.rs/chainmq/latest/chainmq/struct.QueueOptions.html) via **`redis`** ([`RedisClient`](https://docs.rs/chainmq/latest/chainmq/enum.RedisClient.html)). Use the same `key_prefix` (and Redis target) as your workers so the dashboard sees your jobs.

```rust
use std::net::SocketAddr;

use axum::Router;
use chainmq::{
    chainmq_dashboard_router, Queue, QueueOptions, RedisClient, WebUIMountConfig,
};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let queue = Queue::new(QueueOptions {
        redis: RedisClient::Url("redis://127.0.0.1:6379".into()),
        ..Default::default()
    })
    .await?;

    let mount = WebUIMountConfig {
        ui_path: "/dashboard".to_string(),
        ..Default::default()
    };

    let dashboard = chainmq_dashboard_router(queue, mount)?;
    let app = Router::new().nest_service("/dashboard", dashboard);

    // Use `nest_service` (not `nest`) so both `/dashboard` and `/dashboard/` reach the UI; Axum’s
    // `nest` only registers the exact prefix path and does not match a trailing slash on it.

    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await
}
```

If [`WebUIMountConfig::ui_path`](https://docs.rs/chainmq/latest/chainmq/struct.WebUIMountConfig.html) is `"/"`, **merge** the router at the root of your app instead of nesting (avoid path clashes with your other routes).

For a runnable example, see [`examples/web_ui.rs`](./examples/web_ui.rs).

### 3. Actix: `configure`

Enable **`web-ui-actix`**, share one `Arc<tokio::sync::Mutex<Queue>>` across workers, and call [`configure_chainmq_web_ui`](https://docs.rs/chainmq/latest/chainmq/fn.configure_chainmq_web_ui.html) from `App::configure`. You still bind and run [`HttpServer`](https://docs.rs/actix-web/latest/actix_web/struct.HttpServer.html) in **your** binary.

### 4. Open the dashboard

Use the URL your server prints (for example `http://127.0.0.1:8080/dashboard/` when nested at `/dashboard` on port 8080). Unless you set `WebUIMountConfig { auth: None, .. }`, sign-in defaults are **`ChainMQ`** / **`ChainMQ`** until you override `auth`. JSON lives under `{ui_path}/api/...`; those routes are **not** a supported public HTTP API (same-origin browser fetches only).

## Job execution logs

The **Logs** tab on a job reads lines stored in **Redis** (same instance and key prefix as the queue), loaded by the dashboard over its internal JSON routes. They are **not** captured from `println!` or raw stdout: with concurrent jobs in one process, stdout is global and cannot be attributed per job. Use **`tracing`** (`info!`, `debug!`, etc.) inside your job’s `perform` while the worker has entered the `job_execution` span (the library does this around your handler).

**Default:** when you start a `Worker` and the process has **no** global `tracing` subscriber yet, ChainMQ installs one: `EnvFilter` (from `RUST_LOG`, else `info`), stdout formatting, and **`job_logs_layer`** for that worker’s queue. No extra setup is required (see [`examples/worker_main.rs`](./examples/worker_main.rs)).

**Custom subscriber:** if you call `tracing_subscriber::…::init()` (or equivalent) **before** `WorkerBuilder::spawn`, you must add **`chainmq::job_logs_layer`** yourself and pass the same **`Arc<Queue>`** via **`WorkerBuilder::with_shared_queue`**, so the layer and the worker share one queue handle.

Optional: cap retention with **`QueueOptions::job_logs_max_lines`** (default `500`). Logs for a job are removed when the job row is deleted.

## Configuration

### `WebUIMountConfig`

Mount-only settings (no bind address or port in this crate):

```rust
pub struct WebUIMountConfig {
    /// Base path for the UI (e.g. `/dashboard`, `/admin/queues`). Use `/` only if the dashboard
    /// owns those paths on this router.
    pub ui_path: String,
    /// When `Some`, the dashboard requires login (HttpOnly session cookie).
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

### Examples

#### Default (login on, root path in config — nest or merge accordingly)

```rust
let config = WebUIMountConfig::default();
// ui_path default is "/"; merge router at app root or pick a dedicated prefix.
```

#### Custom dashboard path

```rust
let config = WebUIMountConfig {
    ui_path: "/dashboard".to_string(),
    ..Default::default()
};
// app.nest_service("/dashboard", chainmq_dashboard_router(queue, config)?)
```

#### Operator credentials

```rust
use chainmq::{WebUIAuth, WebUIMountConfig};

let config = WebUIMountConfig {
    auth: Some(WebUIAuth {
        username: "operator".into(),
        password: std::env::var("DASHBOARD_PASSWORD").unwrap(),
    }),
    cookie_secure: true,
    ..Default::default()
};
```

Disable login (trusted local use only):

```rust
let config = WebUIMountConfig {
    auth: None,
    ..Default::default()
};
```

## UI files

The dashboard is served from assets **compiled into** the `chainmq` crate (via [`rust-embed`](https://crates.io/crates/rust-embed) from the [`ui/`](./ui/) tree, excluding `README.md`). Files shipped in the binary include:

- `index.html` — main HTML structure
- `app.js` — frontend logic
- `styles.css` — styling
- `favicon.svg` — favicon

**Developing ChainMQ:** edit files under [`ui/`](./ui/) and run `cargo build` so the embed is refreshed. **Depending on `chainmq`:** no extra filesystem layout is required.

The SPA resolves its API base from the browser path (`{ui_path}/api/...`).

## HTTP JSON and the dashboard

The UI issues `fetch` calls to `{ui_path}/api/...` with **credentials** when auth is enabled. Those routes require a **same-origin** (or same-site) browser request (`Sec-Fetch-Site`), so generic HTTP clients get **403 Forbidden**. They are not a stable public API; use the UI or the Rust `Queue` / workers in code.

## Running in production

1. **Reverse proxy**: Terminate TLS in front of your Axum/Actix process.
2. **Authentication**: Set `WebUIMountConfig.auth` with strong credentials, `session_secret: Some([u8; 64])` from a CSPRNG, and `cookie_secure: true` over HTTPS.
3. **Static assets**: Shipped inside the `chainmq` binary; customize in a fork by editing `ui/` and rebuilding.

## Troubleshooting

### UI not loading

- Rebuild after upgrading `chainmq` (embedded assets follow the compiled crate).
- Check the browser console (blocked scripts, mixed content).

### Dashboard data not loading

- Open the UI in a normal browser tab (same origin as the app). `curl` against `/api/...` is intentionally rejected.
- Verify `ui_path` matches how you nest/merge the router (including trailing slash redirects for non-root prefixes).
- Ensure Redis is reachable and the queue matches workers/enqueuers.

### Port already in use

- Change the **listen address in your application** (not in `chainmq`).
