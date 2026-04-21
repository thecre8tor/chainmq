// src/web_ui.rs
//! Web UI server for ChainMQ dashboard
#[cfg(feature = "web-ui")]
use crate::{JobState, Queue};
#[cfg(feature = "web-ui")]
use actix_files;
#[cfg(feature = "web-ui")]
use actix_web::{
    App, HttpResponse, HttpServer, Result as ActixResult, middleware::DefaultHeaders, web,
};
#[cfg(feature = "web-ui")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "web-ui")]
use std::sync::Arc;
#[cfg(feature = "web-ui")]
use tokio::sync::Mutex;

#[derive(Clone)]
struct AppState {
    queue: Arc<Mutex<Queue>>,
}

#[derive(Serialize)]
struct QueueStatsResponse {
    waiting: usize,
    active: usize,
    delayed: usize,
    failed: usize,
    completed: usize,
}

#[derive(Serialize)]
struct JobListResponse {
    jobs: Vec<crate::JobMetadata>,
}

#[derive(Serialize)]
struct QueueListResponse {
    queues: Vec<String>,
}

#[derive(Deserialize)]
struct RetryJobRequest {
    queue_name: String,
}

#[derive(Deserialize)]
struct DeleteJobRequest {
    queue_name: String,
}

#[derive(Deserialize)]
struct CleanQueueRequest {
    queue_name: String,
    state: String,
}

// API: Get all queues
async fn get_queues(state: web::Data<AppState>) -> ActixResult<HttpResponse> {
    let queue = state.queue.lock().await;
    match queue.list_queues().await {
        Ok(queues) => Ok(HttpResponse::Ok().json(QueueListResponse { queues })),
        Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
            "error": e.to_string()
        }))),
    }
}

// API: Get queue stats
async fn get_queue_stats(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> ActixResult<HttpResponse> {
    let queue_name = path.into_inner();
    let queue = state.queue.lock().await;
    match queue.get_stats(&queue_name).await {
        Ok(stats) => Ok(HttpResponse::Ok().json(QueueStatsResponse {
            waiting: stats.waiting,
            active: stats.active,
            delayed: stats.delayed,
            failed: stats.failed,
            completed: stats.completed,
        })),
        Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
            "error": e.to_string()
        }))),
    }
}

// API: List jobs by state
async fn list_jobs(
    state: web::Data<AppState>,
    path: web::Path<(String, String)>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> ActixResult<HttpResponse> {
    let (queue_name, state_str) = path.into_inner();
    let limit = query
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(100);

    let job_state = match state_str.as_str() {
        "waiting" => JobState::Waiting,
        "active" => JobState::Active,
        "delayed" => JobState::Delayed,
        "failed" => JobState::Failed,
        "completed" => JobState::Completed,
        _ => {
            return Ok(HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Invalid state. Must be: waiting, active, delayed, failed, completed"
            })));
        }
    };

    let queue = state.queue.lock().await;
    match queue.list_jobs(&queue_name, job_state, Some(limit)).await {
        Ok(jobs) => Ok(HttpResponse::Ok().json(JobListResponse { jobs })),
        Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
            "error": e.to_string()
        }))),
    }
}

// API: Get job by ID
async fn get_job(state: web::Data<AppState>, path: web::Path<String>) -> ActixResult<HttpResponse> {
    let job_id_str = path.into_inner();
    let queue = state.queue.lock().await;

    match job_id_str.parse::<uuid::Uuid>() {
        Ok(uuid) => {
            let job_id = crate::JobId(uuid);
            match queue.get_job(&job_id).await {
                Ok(Some(job)) => Ok(HttpResponse::Ok().json(job)),
                Ok(None) => Ok(HttpResponse::NotFound().json(serde_json::json!({
                    "error": "Job not found"
                }))),
                Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": e.to_string()
                }))),
            }
        }
        Err(_) => Ok(HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Invalid job ID format"
        }))),
    }
}

// API: Retry a failed job
async fn retry_job(
    state: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<RetryJobRequest>,
) -> ActixResult<HttpResponse> {
    let job_id_str = path.into_inner();
    let queue = state.queue.lock().await;

    match job_id_str.parse::<uuid::Uuid>() {
        Ok(uuid) => {
            let job_id = crate::JobId(uuid);
            match queue.retry_job(&job_id, &body.queue_name).await {
                Ok(()) => Ok(HttpResponse::Ok().json(serde_json::json!({
                    "success": true,
                    "message": "Job retried successfully"
                }))),
                Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": e.to_string()
                }))),
            }
        }
        Err(_) => Ok(HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Invalid job ID format"
        }))),
    }
}

// API: Delete a job (`queue_name` as query param avoids DELETE bodies, which some clients/proxies mishandle)
async fn delete_job(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<DeleteJobRequest>,
) -> ActixResult<HttpResponse> {
    let job_id_str = path.into_inner();
    let queue = state.queue.lock().await;

    match job_id_str.parse::<uuid::Uuid>() {
        Ok(uuid) => {
            let job_id = crate::JobId(uuid);
            match queue.delete_job(&job_id, &query.queue_name).await {
                Ok(()) => Ok(HttpResponse::Ok().json(serde_json::json!({
                    "success": true,
                    "message": "Job deleted successfully"
                }))),
                Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                    "error": e.to_string()
                }))),
            }
        }
        Err(_) => Ok(HttpResponse::BadRequest().json(serde_json::json!({
            "error": "Invalid job ID format"
        }))),
    }
}

// API: Clean queue
async fn clean_queue(
    state: web::Data<AppState>,
    body: web::Json<CleanQueueRequest>,
) -> ActixResult<HttpResponse> {
    let queue = state.queue.lock().await;
    let mut deleted_count = 0;

    let states_to_clean: Vec<JobState> = if body.state.to_lowercase() == "all" {
        vec![
            JobState::Waiting,
            JobState::Delayed,
            JobState::Failed,
            JobState::Completed,
        ]
    } else {
        match body.state.to_lowercase().as_str() {
            "waiting" => vec![JobState::Waiting],
            "delayed" => vec![JobState::Delayed],
            "failed" => vec![JobState::Failed],
            "completed" => vec![JobState::Completed],
            _ => {
                return Ok(HttpResponse::BadRequest().json(serde_json::json!({
                    "error": "Invalid state. Must be: waiting, delayed, failed, completed, or all"
                })));
            }
        }
    };

    for job_state in states_to_clean {
        match queue
            .list_jobs(&body.queue_name, job_state, Some(10000))
            .await
        {
            Ok(jobs) => {
                for job in jobs {
                    if let Err(e) = queue.delete_job(&job.id, &body.queue_name).await {
                        eprintln!("Failed to delete job {}: {}", job.id, e);
                    } else {
                        deleted_count += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed to list jobs for cleaning: {}", e);
            }
        }
    }

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "message": format!("Cleaned {} jobs", deleted_count),
        "deleted_count": deleted_count
    })))
}

// API: Recover stalled jobs
async fn recover_stalled(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> ActixResult<HttpResponse> {
    let queue_name = path.into_inner();
    let queue = state.queue.lock().await;

    let max_stalled_secs = 60; // Default

    match queue
        .recover_stalled_jobs(&queue_name, max_stalled_secs)
        .await
    {
        Ok(recovered_count) => Ok(HttpResponse::Ok().json(serde_json::json!({
            "success": true,
            "message": format!("Recovered {} stalled jobs", recovered_count),
            "recovered_count": recovered_count
        }))),
        Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
            "error": e.to_string()
        }))),
    }
}

// API: Process delayed jobs
async fn process_delayed(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> ActixResult<HttpResponse> {
    let queue_name = path.into_inner();
    let queue = state.queue.lock().await;

    match queue.process_delayed(&queue_name).await {
        Ok(moved_count) => Ok(HttpResponse::Ok().json(serde_json::json!({
            "success": true,
            "message": format!("Moved {} delayed jobs to waiting", moved_count),
            "moved_count": moved_count
        }))),
        Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
            "error": e.to_string()
        }))),
    }
}

/// Static files (`index.html`, `app.js`, `styles.css`) are served from this path
/// relative to the process working directory. Not configurable via the public API.
const UI_STATIC_DIR: &str = "./ui";

/// Configuration for the web UI server
#[cfg(feature = "web-ui")]
#[derive(Debug, Clone)]
pub struct WebUIConfig {
    /// Port to bind the server to
    pub port: u16,
    /// Base path for the UI (e.g., "/dashboard", "/admin/queues")
    /// The API will be available at `{ui_path}/api`
    pub ui_path: String,
}

impl Default for WebUIConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            ui_path: "/".to_string(),
        }
    }
}

/// Start the web UI server with default settings (port 8080, path "/")
/// This function starts the server and blocks until Ctrl+C is pressed.
/// All logging and shutdown handling is managed internally.
///
/// # Example
/// ```no_run
/// use chainmq::{Queue, QueueOptions, start_web_ui_simple};
///
/// # async fn example() -> anyhow::Result<()> {
/// let queue = Queue::new(QueueOptions::default()).await?;
/// start_web_ui_simple(queue).await?;
/// // Function blocks here until Ctrl+C is pressed
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "web-ui")]
pub async fn start_web_ui_simple(queue: Queue) -> std::io::Result<()> {
    let config = WebUIConfig::default();
    let server = start_web_ui(queue, config).await?;

    // Spawn server in background
    tokio::spawn(async move {
        if let Err(e) = server.await {
            eprintln!("[web-ui] Server error: {}", e);
        }
    });

    // Wait for shutdown signal (this blocks until Ctrl+C)
    if let Err(e) = tokio::signal::ctrl_c().await {
        eprintln!("[web-ui] Failed to listen for shutdown signal: {}", e);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Failed to listen for shutdown signal: {}", e),
        ));
    }

    println!("\n[web-ui] Shutting down...");
    Ok(())
}

/// Start the web UI server
///
/// # Example
/// ```no_run
/// use chainmq::{Queue, QueueOptions, start_web_ui, WebUIConfig};
///
/// # async fn example() -> anyhow::Result<()> {
/// let queue = Queue::new(QueueOptions::default()).await?;
/// let config = WebUIConfig {
///     port: 8080,
///     ui_path: "/dashboard".to_string(),
///     ..Default::default()
/// };
///
/// // Start the server (this will run until the process is terminated)
/// start_web_ui(queue, config).await?.await?;
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "web-ui")]
pub async fn start_web_ui(
    queue: Queue,
    config: WebUIConfig,
) -> std::io::Result<actix_web::dev::Server> {
    let app_state = AppState {
        queue: Arc::new(Mutex::new(queue)),
    };

    let ui_path = config.ui_path.clone();
    let ui_dir = UI_STATIC_DIR.to_string();
    let port = config.port;

    let api_path = if ui_path == "/" {
        "/api".to_string()
    } else {
        format!("{}/api", ui_path)
    };

    // Clone for use in closure and println
    let ui_path_for_closure = ui_path.clone();
    let ui_dir_for_closure = ui_dir.clone();
    let api_path_for_closure = api_path.clone();
    let api_path_for_print = api_path.clone();

    let server = HttpServer::new(move || {
        let api_path = api_path_for_closure.clone();
        App::new()
            .wrap(
                DefaultHeaders::new().add((
                    "Cache-Control",
                    "no-store, max-age=0, must-revalidate",
                )),
            )
            .app_data(web::Data::new(app_state.clone()))
            // Register API routes first (more specific)
            .route(&format!("{}/queues", api_path), web::get().to(get_queues))
            .route(
                &format!("{}/queues/{{queue_name}}/stats", api_path),
                web::get().to(get_queue_stats),
            )
            .route(
                &format!("{}/queues/{{queue_name}}/jobs/{{state}}", api_path),
                web::get().to(list_jobs),
            )
            .route(
                &format!("{}/jobs/{{job_id}}", api_path),
                web::get().to(get_job),
            )
            .route(
                &format!("{}/jobs/{{job_id}}/retry", api_path),
                web::post().to(retry_job),
            )
            .route(
                &format!("{}/jobs/{{job_id}}/delete", api_path),
                web::delete().to(delete_job),
            )
            .route(
                &format!("{}/queues/clean", api_path),
                web::post().to(clean_queue),
            )
            .route(
                &format!("{}/queues/{{queue_name}}/recover-stalled", api_path),
                web::post().to(recover_stalled),
            )
            .route(
                &format!("{}/queues/{{queue_name}}/process-delayed", api_path),
                web::post().to(process_delayed),
            )
            // Register static files last (catch-all for non-API routes)
            .service(
                actix_files::Files::new(&ui_path_for_closure, &ui_dir_for_closure)
                    .index_file("index.html")
                    .prefer_utf8(true),
            )
    })
    .bind(("127.0.0.1", port))?
    .run();

    println!(
        "[web-ui] ChainMQ Web UI started on http://127.0.0.1:{}{}",
        port, ui_path
    );
    println!(
        "[web-ui] API available at http://127.0.0.1:{}{}",
        port, api_path_for_print
    );
    println!(
        "[web-ui] Open http://127.0.0.1:{}{} in your browser to view the dashboard",
        port, ui_path
    );
    println!(
        "[web-ui] Static files: {} (relative to process CWD — use repo root so UI edits are picked up)",
        UI_STATIC_DIR
    );
    println!("[web-ui] Press Ctrl+C to stop the server");

    Ok(server)
}
