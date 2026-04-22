// Example: Axum server with the ChainMQ dashboard nested at a custom path.

use std::net::SocketAddr;

use axum::Router;
use chainmq::{Queue, QueueOptions, RedisClient, WebUIMountConfig, chainmq_dashboard_router};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::try_init().ok();

    let options = QueueOptions {
        redis: RedisClient::Url("redis://127.0.0.1:6370".into()),
        ..Default::default()
    };
    let queue = Queue::new(options).await?;

    let mount = WebUIMountConfig {
        ui_path: "/dashboard".to_string(),
        ..Default::default()
    };

    let dashboard = chainmq_dashboard_router(queue, mount)?;

    let app = Router::new().nest_service("/dashboard", dashboard);

    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Dashboard at http://127.0.0.1:8080/dashboard/");
    axum::serve(listener, app).await?;
    Ok(())
}
