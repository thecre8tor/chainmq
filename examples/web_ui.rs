use actix_web::{
    App, HttpResponse, HttpServer, Result as ActixResult, middleware::DefaultHeaders, web,
};
use chainmq::{JobState, Queue, QueueOptions};
use redis::Client as RedisClient;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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
    jobs: Vec<chainmq::JobMetadata>,
}

#[derive(Serialize)]
struct JobLogLineDto {
    ts: String,
    level: String,
    message: String,
}

#[derive(Serialize)]
struct JobLogsResponse {
    lines: Vec<JobLogLineDto>,
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
struct JobLogsQuery {
    limit: Option<usize>,
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
    let state_for_log = format!("{:?}", job_state);
    match queue.list_jobs(&queue_name, job_state, Some(limit)).await {
        Ok(jobs) => {
            eprintln!(
                "Found {} jobs in state {} for queue {}",
                jobs.len(),
                state_for_log,
                queue_name
            );
            Ok(HttpResponse::Ok().json(JobListResponse { jobs }))
        }
        Err(e) => {
            eprintln!("Error listing jobs: {}", e);
            Ok(HttpResponse::InternalServerError().json(serde_json::json!({
                "error": e.to_string()
            })))
        }
    }
}

// API: Get job by ID
async fn get_job(state: web::Data<AppState>, path: web::Path<String>) -> ActixResult<HttpResponse> {
    let job_id_str = path.into_inner();
    let job_id = match job_id_str.parse::<uuid::Uuid>() {
        Ok(uuid) => chainmq::JobId(uuid),
        Err(_) => {
            return Ok(HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Invalid job ID format"
            })));
        }
    };

    let queue = state.queue.lock().await;
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

async fn get_job_logs(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<JobLogsQuery>,
) -> ActixResult<HttpResponse> {
    let job_id_str = path.into_inner();
    let job_id = match job_id_str.parse::<uuid::Uuid>() {
        Ok(uuid) => chainmq::JobId(uuid),
        Err(_) => {
            return Ok(HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Invalid job ID format"
            })));
        }
    };
    let limit = query.limit.unwrap_or(200).clamp(1, 500);
    let queue = state.queue.lock().await;
    match queue.get_job_logs(&job_id, limit).await {
        Ok(lines) => {
            let lines: Vec<JobLogLineDto> = lines
                .into_iter()
                .map(|l| JobLogLineDto {
                    ts: l.ts,
                    level: l.level,
                    message: l.message,
                })
                .collect();
            Ok(HttpResponse::Ok().json(JobLogsResponse { lines }))
        }
        Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
            "error": e.to_string()
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
    let job_id = match job_id_str.parse::<uuid::Uuid>() {
        Ok(uuid) => chainmq::JobId(uuid),
        Err(_) => {
            return Ok(HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Invalid job ID format"
            })));
        }
    };

    let queue = state.queue.lock().await;
    match queue.retry_job(&job_id, &body.queue_name).await {
        Ok(_) => Ok(HttpResponse::Ok().json(serde_json::json!({
            "success": true,
            "message": "Job queued for retry"
        }))),
        Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
            "error": e.to_string()
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
    let job_id = match job_id_str.parse::<uuid::Uuid>() {
        Ok(uuid) => chainmq::JobId(uuid),
        Err(_) => {
            return Ok(HttpResponse::BadRequest().json(serde_json::json!({
                "error": "Invalid job ID format"
            })));
        }
    };

    let queue = state.queue.lock().await;
    match queue.delete_job(&job_id, &query.queue_name).await {
        Ok(_) => Ok(HttpResponse::Ok().json(serde_json::json!({
            "success": true,
            "message": "Job deleted"
        }))),
        Err(e) => Ok(HttpResponse::InternalServerError().json(serde_json::json!({
            "error": e.to_string()
        }))),
    }
}

// API: Clean queue
#[derive(Deserialize)]
struct CleanQueueRequest {
    queue_name: String,
    state: Option<String>,
}

async fn clean_queue(
    state: web::Data<AppState>,
    body: web::Json<CleanQueueRequest>,
) -> ActixResult<HttpResponse> {
    let queue = state.queue.lock().await;
    let state_filter = body.state.as_deref();

    let mut deleted_count = 0;

    // Get all jobs for the specified state(s)
    // Note: We don't clean active jobs as they're being processed
    let states_to_clean = if state_filter == Some("all") || state_filter.is_none() {
        vec![
            chainmq::JobState::Waiting,
            chainmq::JobState::Delayed,
            chainmq::JobState::Failed,
            chainmq::JobState::Completed,
        ]
    } else {
        match state_filter {
            Some("waiting") => vec![chainmq::JobState::Waiting],
            Some("active") => {
                return Ok(HttpResponse::BadRequest().json(serde_json::json!({
                    "error": "Cannot clean active jobs. They are currently being processed."
                })));
            }
            Some("delayed") => vec![chainmq::JobState::Delayed],
            Some("failed") => vec![chainmq::JobState::Failed],
            Some("completed") => vec![chainmq::JobState::Completed],
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

    // Use max_stalled_interval from queue options (default 30 seconds)
    // But also check job-specific timeouts, so use a reasonable default
    let max_stalled_secs = 60; // 60 seconds default - jobs without timeout will use this

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

// API: Process delayed jobs (move them to waiting when due)
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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::try_init().ok();

    // Get Redis URL from environment or use default
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6370".to_string());

    println!("Starting ChainMQ Web UI on http://127.0.0.1:8080");
    println!("Connecting to Redis at: {}", redis_url);

    // Create Redis client and queue
    let redis_client = RedisClient::open(redis_url.as_str()).expect("Failed to connect to Redis");

    let queue_options = QueueOptions {
        redis_instance: Some(redis_client),
        ..Default::default()
    };

    let queue = Queue::new(queue_options)
        .await
        .expect("Failed to create queue");

    let app_state = AppState {
        queue: Arc::new(Mutex::new(queue)),
    };

    // Start HTTP server
    HttpServer::new(move || {
        App::new()
            .wrap(
                DefaultHeaders::new()
                    .add(("Cache-Control", "no-store, max-age=0, must-revalidate")),
            )
            .app_data(web::Data::new(app_state.clone()))
            .route("/api/queues", web::get().to(get_queues))
            .route(
                "/api/queues/{queue_name}/stats",
                web::get().to(get_queue_stats),
            )
            .route(
                "/api/queues/{queue_name}/jobs/{state}",
                web::get().to(list_jobs),
            )
            .route("/api/jobs/{job_id}/logs", web::get().to(get_job_logs))
            .route("/api/jobs/{job_id}", web::get().to(get_job))
            .route("/api/jobs/{job_id}/retry", web::post().to(retry_job))
            .route("/api/jobs/{job_id}/delete", web::delete().to(delete_job))
            .route("/api/queues/clean", web::post().to(clean_queue))
            .route(
                "/api/queues/{queue_name}/recover-stalled",
                web::post().to(recover_stalled),
            )
            .route(
                "/api/queues/{queue_name}/process-delayed",
                web::post().to(process_delayed),
            )
            .service(actix_files::Files::new("/", "./ui").index_file("index.html"))
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
}
