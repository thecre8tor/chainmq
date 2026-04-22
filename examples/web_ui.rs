//! Minimal Axum host: bind your own address and nest the ChainMQ dashboard.
//!
//! Environment:
//! - `REDIS_URL` — Redis connection (default `redis://127.0.0.1:6379`)
//!
//! By default the dashboard requires sign-in (**`ChainMQ` / `ChainMQ`**) unless you change
//! [`WebUIMountConfig::auth`].

use std::net::SocketAddr;

use axum::{Router, routing::get};
use chainmq::{Queue, QueueOptions, RedisClient, WebUIMountConfig, chainmq_dashboard_router};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt::try_init().ok();

    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

    let queue = Queue::new(QueueOptions {
        redis: RedisClient::Url(redis_url),
        ..Default::default()
    })
    .await
    .expect("Failed to create queue");

    let mount = WebUIMountConfig {
        ui_path: "/dashboard".to_string(),
        ..Default::default()
    };

    let dashboard = chainmq_dashboard_router(queue, mount).expect("dashboard router");

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest_service("/dashboard", dashboard);

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!(
        "Listening on http://{addr} — open http://127.0.0.1:{port}/dashboard/ in your browser"
    );
    axum::serve(listener, app).await
}
