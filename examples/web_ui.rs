//! Standalone Web UI process (same server as `chainmq::start_web_ui`).
//!
//! By default the dashboard requires sign-in (see [`WebUIConfig::default`] in the library: username
//! and password **`ChainMQ`**). Override credentials in code, for example:
//!
//! ```ignore
//! WebUIConfig {
//!     auth: Some(WebUIAuth {
//!         username: "admin".into(),
//!         password: std::env::var("DASHBOARD_PASSWORD").expect("DASHBOARD_PASSWORD"),
//!     }),
//!     ..WebUIConfig::default()
//! }
//! ```
//!
//! Environment:
//! - `REDIS_URL` — Redis connection (default `redis://127.0.0.1:6370` to match other examples)
//! - Optional: set `auth: None` on [`WebUIConfig`] if you want no login (not recommended for exposed hosts).

use chainmq::{Queue, QueueOptions, RedisClient, WebUIConfig, start_web_ui};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt::try_init().ok();

    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6370".to_string());

    println!("Connecting to Redis at: {}", redis_url);

    let queue = Queue::new(QueueOptions {
        redis: RedisClient::Url(redis_url),
        ..Default::default()
    })
    .await
    .expect("Failed to create queue");

    let config = WebUIConfig {
        port: 8080,
        ..Default::default()
    };

    println!(
        "Starting ChainMQ Web UI on http://127.0.0.1:{} (sign in with default ChainMQ / ChainMQ unless you changed WebUIConfig.auth)",
        config.port
    );

    start_web_ui(queue, config).await?.await
}
