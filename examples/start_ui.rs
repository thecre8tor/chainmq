// examples/start_ui.rs
// Example showing how to start the web UI automatically with a custom path

use chainmq::{Queue, QueueOptions, WebUIConfig, start_web_ui};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::try_init().ok();

    // Create queue with your Redis connection
    let options = QueueOptions {
        redis_url: "redis://127.0.0.1:6370".to_string(),
        ..Default::default()
    };
    let queue = Queue::new(options).await?;

    // Configure the web UI
    let ui_config = WebUIConfig {
        port: 8080,
        ui_path: "/dashboard".to_string(),
        ..Default::default()
    };

    println!("Starting ChainMQ Web UI...");
    println!(
        "Access the dashboard at: http://127.0.0.1:{}{}",
        ui_config.port, ui_config.ui_path
    );

    // Start the web UI server
    // This will run until the process is terminated
    start_web_ui(queue, ui_config).await?.await?;

    Ok(())
}
