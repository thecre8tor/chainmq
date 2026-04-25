# ChainMQ Web UI

The ChainMQ Web UI provides a modern dashboard for monitoring and managing your job queues. Use **one** [`Queue`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html) configured for your Redis instance and `key_prefix`; it lists and manages **every** logical queue name (`Job::queue_name()`) in that namespace.

The library **does not start an HTTP server**. You choose the host, port, and TLS on your Axum or Actix app, then **mount** the dashboard routes at your chosen base path.

## Features

- 🎨 Modern, classy UI with light/dark mode
- 📊 Real-time queue statistics
- 🔍 Job search and filtering
- 📄 Pagination for large job lists
- ⚡ Queue actions (clean, recover stalled, process delayed, **process repeat**, **pause / resume** queue)
- 🔄 Auto-refresh every 3 seconds
- 📱 Responsive design
- 📜 Per-job **Activity** tab (queue events stream; see [Lifecycle events](#lifecycle-events))
- 🖥️ **Redis** button on the queue toolbar opens a modal with memory / `INFO` snapshot (used vs cap or host RAM), distinct from per-queue job counts
- 📝 Optional per-job execution logs via `tracing` + `job_logs_layer` when enabled on the worker ([Job execution logs](#job-execution-logs-opt-in))

## Responsive layout

The UI scales from **desktop** (persistent sidebar, wide tables, multi-column job detail) to **mobile** (top chrome with menu drawer, stacked stat cards, single-column job detail and activity). Source files for the dashboard live under [`ui/`](./ui/) in this repository; at **compile time** they are embedded into the `chainmq` library binary (see [UI files](#ui-files)).

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
chainmq = { version = "1.3.2", default-features = false }
```

Actix-only consumers:

```toml
chainmq = { version = "1.3.2", default-features = false, features = ["web-ui-actix"] }
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

For a runnable example, see [`examples/web_ui/web_ui.rs`](./examples/web_ui/web_ui.rs).

### Repeat schedules, process repeat, and queue pause

- **Pause / resume:** toolbar buttons call `POST …/api/queues/{queue}/pause` and `…/resume`; the UI refreshes paused state from `GET …/paused`.
- **Process repeat:** runs `POST …/api/queues/{queue}/process-repeat`, which calls [`Queue::process_repeat`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html#method.process_repeat). This is queue-native and does not require extra dashboard registry wiring.
- **List / add / remove repeats:** `GET …/repeats`, `POST …/repeats/interval`, `POST …/repeats/cron`, `DELETE …/repeats/{schedule_id}` (see [`AddRepeatIntervalRequest`](https://docs.rs/chainmq/latest/chainmq/struct.AddRepeatIntervalRequest.html) / [`AddRepeatCronRequest`](https://docs.rs/chainmq/latest/chainmq/struct.AddRepeatCronRequest.html) in the crate docs). You can drive these from your own tools; the bundled UI currently exposes pause, resume, and **Process repeat** on the queue toolbar.

### 3. Actix: `configure`

Enable **`web-ui-actix`**, share one `Arc<tokio::sync::Mutex<Queue>>` across workers, and call [`configure_chainmq_web_ui`](https://docs.rs/chainmq/latest/chainmq/fn.configure_chainmq_web_ui.html) from `App::configure`. You still bind and run [`HttpServer`](https://docs.rs/actix-web/latest/actix_web/struct.HttpServer.html) in **your** binary.

### 4. Open the dashboard

Use the URL your server prints (for example `http://127.0.0.1:8080/dashboard/` when nested at `/dashboard` on port 8080). Unless you set `WebUIMountConfig { auth: None, .. }`, sign-in defaults are **`ChainMQ`** / **`ChainMQ`** until you override `auth`. JSON lives under `{ui_path}/api/...`; those routes are **not** a supported public HTTP API (same-origin browser fetches only).

## Lifecycle events

Primary observability: each logical queue appends JSON to a capped Redis Stream and publishes the **same JSON string** on `{key_prefix}:events:{queue_name}`.

- **Stream key:** `{key_prefix}:events:{queue_name}:stream` (approximate length `QueueOptions::events_stream_max_len`, `XADD MAXLEN ~`).
- **Event payload:** JSON object with at least `type` and `ts` (milliseconds); optional `jobId`, `data`, `detail` depending on the event.

Internal dashboard routes (session-protected when auth is on):

| Method | Path (under `{ui_path}/api`)            | Purpose                                                                                                                                                                                           |
| ------ | --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET`  | `/queues/{queue_name}/events?limit=100` | Recent events (newest first) from the stream                                                                                                                                                      |
| `GET`  | `/queues/{queue_name}/events/live`      | **SSE** (`text/event-stream`): subscribe to the pub/sub channel for live JSON lines                                                                                                               |
| `GET`  | `/redis/stats`                          | Whitelisted `INFO` fields for the **shared** instance (includes `used_memory` / `used_memory_human`, `maxmemory`, `total_system_memory`, `used_memory_rss`, version, uptime, clients, ops/sec, …) |

**SSE caveat:** `events/live` is for custom dashboards or tools that open an [`EventSource`](https://developer.mozilla.org/en-US/docs/Web/API/EventSource); it needs a dedicated Redis pub/sub connection from a [`redis::Client`](https://docs.rs/redis/latest/redis/struct.Client.html). If the app’s [`Queue`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html) was built with [`RedisClient::Manager`](https://docs.rs/chainmq/latest/chainmq/enum.RedisClient.html), the history endpoint still works; the live route returns **503** with JSON. Prefer `RedisClient::Url` or `Client` if you embed live SSE.

The UI polls queue job counts every few seconds. **`GET /redis/stats`** is called when you open the **Redis** modal (fresh fetch each time you open it).

## Job execution logs (opt-in)

Redis-backed **tracing** log lines for the optional `GET …/jobs/{id}/logs` API are **not** captured from `println!` or raw stdout. Use **`tracing`** (`info!`, `debug!`, etc.) inside `perform` while the worker has entered the `job_execution` span.

**Default:** workers do **not** install a global subscriber or `job_logs_layer`. Use queue/stream events and the **Activity** tab first.

**Enable Redis job logs:** call **`WorkerBuilder::with_tracing_job_logs(true)`** (or set **`WorkerConfig::tracing_job_logs`**) so that, when the process still has no global `tracing` subscriber at worker creation, ChainMQ installs `EnvFilter` (from `RUST_LOG`, else `info`), stdout formatting, and **`job_logs_layer`** for that worker’s queue. See [`examples/getting_started/worker_main.rs`](./examples/getting_started/worker_main.rs).

**Custom subscriber:** if you call `tracing_subscriber::…::init()` **before** `WorkerBuilder::spawn`, you must add **`chainmq::job_logs_layer`** yourself and pass the same **`Arc<Queue>`** via **`WorkerBuilder::with_shared_queue`**, so the layer and the worker share one queue handle.

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
    /// Session signing master key: provide exactly 32 bytes (`cookie::Key::derive_from`) or 64 bytes (used directly).
    /// If `None` while `auth` is set, a fixed **dev-only** key is used.
    pub session_secret: Option<Vec<u8>>,
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
2. **Authentication**: Set `WebUIMountConfig.auth` with strong credentials, `session_secret: Some(secret_bytes)` where `secret_bytes.len()` is `32` or `64` from a CSPRNG, and `cookie_secure: true` over HTTPS.
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
