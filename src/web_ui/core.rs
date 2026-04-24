//! Shared dashboard types, static assets, path helpers, and queue JSON handlers.

use crate::{JobOptions, JobRegistry, JobState, Queue, RepeatCatchUp};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(rust_embed::RustEmbed)]
#[folder = "ui"]
#[exclude = "README.md"]
pub struct UiAssets;

/// Session payload key set after successful dashboard login.
pub const SESSION_AUTH_KEY: &str = "chainmq_ui_authenticated";

/// Fixed 64-byte development signing key when [`WebUIMountConfig::session_secret`] is `None`.
pub const DEFAULT_WEB_UI_SESSION_SECRET: &[u8; 64] =
    b"chainmq-web-ui-DEV-SESSION-KEY-64B-DO-NOT-USE-IN-PRODUCTION!!!!!";

#[derive(Clone)]
pub struct UiLoginRuntime {
    pub expected_username: String,
    pub password_hash: String,
}

#[derive(Serialize)]
pub struct QueueStatsResponse {
    pub waiting: usize,
    pub active: usize,
    pub delayed: usize,
    pub failed: usize,
    pub completed: usize,
}

#[derive(Serialize)]
pub struct JobListResponse {
    pub jobs: Vec<crate::JobMetadata>,
}

#[derive(Serialize)]
pub struct JobLogLineDto {
    pub ts: String,
    pub level: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct JobLogsResponse {
    pub lines: Vec<JobLogLineDto>,
}

#[derive(Serialize)]
pub struct QueueListResponse {
    pub queues: Vec<String>,
}

#[derive(Deserialize)]
pub struct RetryJobRequest {
    pub queue_name: String,
}

#[derive(Deserialize)]
pub struct DeleteJobRequest {
    pub queue_name: String,
}

#[derive(Deserialize)]
pub struct CleanQueueRequest {
    pub queue_name: String,
    pub state: String,
}

#[derive(Deserialize)]
pub struct JobLogsQuery {
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct QueueEventsQuery {
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct AuthSessionResponse {
    pub auth_enabled: bool,
    pub authenticated: bool,
}

/// Username and password for the built-in dashboard login.
///
/// Default credentials are **`ChainMQ` / `ChainMQ`**. Override in production.
#[derive(Clone)]
pub struct WebUIAuth {
    pub username: String,
    pub password: String,
}

impl Default for WebUIAuth {
    fn default() -> Self {
        Self {
            username: "ChainMQ".to_string(),
            password: "ChainMQ".to_string(),
        }
    }
}

/// Mount-only configuration: base path and session/auth options.
///
/// **Host, port, and TLS** are defined by your application when you bind the server.
/// With the Axum integration, nest the returned router at [`Self::ui_path`] (normalized, e.g.
/// `/dashboard`), or merge at `/` when [`Self::ui_path`] is `"/"`.
#[derive(Clone)]
pub struct WebUIMountConfig {
    /// Base path for the UI (e.g. `/dashboard`, `/admin/queues`). Use `/` only if the dashboard
    /// is the only consumer of those paths on this router.
    pub ui_path: String,
    /// When `Some`, the dashboard requires login (HttpOnly session cookie).
    pub auth: Option<WebUIAuth>,
    /// Master key (64 bytes) used to sign the session cookie. If `None` while [`Self::auth`] is
    /// `Some`, a **fixed development key** is used.
    pub session_secret: Option<[u8; 64]>,
    /// `Secure` flag on the session cookie (use `true` behind HTTPS).
    pub cookie_secure: bool,
    /// When `Some`, repeat promotion and “add repeat” APIs can materialize jobs by type name.
    pub job_registry: Option<Arc<JobRegistry>>,
}

impl Default for WebUIMountConfig {
    fn default() -> Self {
        Self {
            ui_path: "/".to_string(),
            auth: Some(WebUIAuth::default()),
            session_secret: None,
            cookie_secure: false,
            job_registry: None,
        }
    }
}

/// Returns signing key material for cookie sessions.
pub fn session_signing_key_material(config: &WebUIMountConfig) -> [u8; 64] {
    match &config.session_secret {
        Some(k) => *k,
        None => {
            if config.auth.is_some() {
                tracing::warn!(
                    "[web-ui] Using built-in development session signing key; set WebUIMountConfig.session_secret in production"
                );
            }
            *DEFAULT_WEB_UI_SESSION_SECRET
        }
    }
}

pub fn session_cookie_path(ui_path: &str) -> String {
    let p = ui_path.trim();
    if p.is_empty() || p == "/" {
        "/".to_string()
    } else {
        p.trim_end_matches('/').to_string()
    }
}

/// Normalized URL prefix (`/` or `/dashboard`, never trailing slash except `/`).
pub fn normalize_static_url_prefix(ui_path: &str) -> String {
    let p = ui_path.trim();
    if p.is_empty() || p == "/" {
        "/".to_string()
    } else {
        format!(
            "/{}",
            p.trim().trim_start_matches('/').trim_end_matches('/')
        )
    }
}

/// Path seen by handlers inside [`axum::Router::nest`] is **stripped** of the mount prefix.
/// Reconstruct the browser path so [`embedded_asset_rel_key`] and trailing-slash redirects work.
pub fn full_path_for_embedded_request(request_path: &str, static_prefix: &str) -> String {
    let request_path = request_path.trim();
    if static_prefix == "/" {
        return if request_path.is_empty() {
            "/".to_string()
        } else {
            request_path.to_string()
        };
    }
    let prefix = static_prefix.trim_end_matches('/');
    let starts_with_mount = request_path.starts_with(prefix)
        && (request_path.len() == prefix.len()
            || request_path
                .as_bytes()
                .get(prefix.len())
                .is_some_and(|b| *b == b'/'));
    if starts_with_mount {
        return request_path.to_string();
    }
    // Nested: e.g. mount `/dashboard`, inner path `/` or `/app.js`
    if request_path.is_empty() || request_path == "/" {
        format!("{}/", prefix)
    } else if request_path.starts_with('/') {
        format!("{}{}", prefix, request_path)
    } else {
        format!("{}/{}", prefix, request_path)
    }
}

pub fn embedded_asset_rel_key(full_path: &str, prefix: &str) -> Option<String> {
    if full_path.contains("..") {
        return None;
    }
    let rest = if prefix == "/" {
        full_path.trim_start_matches('/')
    } else if let Some(stripped) = full_path.strip_prefix(prefix) {
        stripped.trim_start_matches('/')
    } else {
        return None;
    };
    if rest.is_empty() {
        return Some("index.html".to_string());
    }
    if rest.contains('/') {
        return None;
    }
    Some(rest.to_string())
}

pub fn embedded_content_type(rel: &str) -> &'static str {
    if rel.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if rel.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if rel.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if rel.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    }
}

pub fn is_ui_auth_public_route(method: &str, path: &str) -> bool {
    (method == "GET" && path.ends_with("/auth/session"))
        || (method == "POST" && path.ends_with("/auth/login"))
        || (method == "POST" && path.ends_with("/auth/logout"))
}

pub fn build_login_runtime(auth: &WebUIAuth) -> std::io::Result<Arc<UiLoginRuntime>> {
    let hash = bcrypt::hash(&auth.password, bcrypt::DEFAULT_COST).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("failed to hash dashboard password: {e}"),
        )
    })?;
    Ok(Arc::new(UiLoginRuntime {
        expected_username: auth.username.clone(),
        password_hash: hash,
    }))
}

pub fn json_reauth_value(message: &str) -> serde_json::Value {
    serde_json::json!({
        "error": message,
        "reauth": true,
    })
}

// --- Queue API handlers (return JSON value + HTTP status) ---

pub async fn api_get_queues(queue: &Queue) -> (http::StatusCode, serde_json::Value) {
    match queue.list_queues().await {
        Ok(queues) => (
            http::StatusCode::OK,
            serde_json::to_value(QueueListResponse { queues }).unwrap_or_default(),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_list_queue_events(
    queue: &Queue,
    queue_name: &str,
    limit: usize,
) -> (http::StatusCode, serde_json::Value) {
    match queue.read_queue_events(queue_name, limit).await {
        Ok(events) => (
            http::StatusCode::OK,
            serde_json::json!({ "events": events }),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_get_redis_server_stats(queue: &Queue) -> (http::StatusCode, serde_json::Value) {
    match queue.redis_server_metrics_json().await {
        Ok(v) => (http::StatusCode::OK, v),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_get_queue_stats(
    queue: &Queue,
    queue_name: &str,
) -> (http::StatusCode, serde_json::Value) {
    match queue.get_stats(queue_name).await {
        Ok(stats) => (
            http::StatusCode::OK,
            serde_json::to_value(QueueStatsResponse {
                waiting: stats.waiting,
                active: stats.active,
                delayed: stats.delayed,
                failed: stats.failed,
                completed: stats.completed,
            })
            .unwrap_or_default(),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub fn parse_job_state(state_str: &str) -> Result<JobState, serde_json::Value> {
    match state_str {
        "waiting" => Ok(JobState::Waiting),
        "active" => Ok(JobState::Active),
        "delayed" => Ok(JobState::Delayed),
        "failed" => Ok(JobState::Failed),
        "completed" => Ok(JobState::Completed),
        _ => Err(serde_json::json!({
            "error": "Invalid state. Must be: waiting, active, delayed, failed, completed"
        })),
    }
}

/// Build [`crate::JobId`] from a URL path segment (percent-decoded by the framework).
fn job_id_from_path(raw: &str) -> Result<crate::JobId, &'static str> {
    let t = raw.trim();
    if t.is_empty() {
        Err("job id must not be empty")
    } else {
        Ok(crate::JobId(t.to_string()))
    }
}

pub async fn api_list_jobs(
    queue: &Queue,
    queue_name: &str,
    state_str: &str,
    limit: usize,
) -> (http::StatusCode, serde_json::Value) {
    let job_state = match parse_job_state(state_str) {
        Ok(s) => s,
        Err(j) => return (http::StatusCode::BAD_REQUEST, j),
    };
    match queue.list_jobs(queue_name, job_state, Some(limit)).await {
        Ok(jobs) => (
            http::StatusCode::OK,
            serde_json::to_value(JobListResponse { jobs }).unwrap_or_default(),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_get_job(queue: &Queue, job_id_str: &str) -> (http::StatusCode, serde_json::Value) {
    let job_id = match job_id_from_path(job_id_str) {
        Ok(id) => id,
        Err(msg) => {
            return (
                http::StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": msg }),
            );
        }
    };
    match queue.get_job(&job_id).await {
        Ok(Some(job)) => (
            http::StatusCode::OK,
            serde_json::to_value(job).unwrap_or_default(),
        ),
        Ok(None) => (
            http::StatusCode::NOT_FOUND,
            serde_json::json!({ "error": "Job not found" }),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_get_job_logs(
    queue: &Queue,
    job_id_str: &str,
    limit: usize,
) -> (http::StatusCode, serde_json::Value) {
    let job_id = match job_id_from_path(job_id_str) {
        Ok(id) => id,
        Err(msg) => {
            return (
                http::StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": msg }),
            );
        }
    };
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
            (
                http::StatusCode::OK,
                serde_json::to_value(JobLogsResponse { lines }).unwrap_or_default(),
            )
        }
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_retry_job(
    queue: &Queue,
    job_id_str: &str,
    queue_name: &str,
) -> (http::StatusCode, serde_json::Value) {
    let job_id = match job_id_from_path(job_id_str) {
        Ok(id) => id,
        Err(msg) => {
            return (
                http::StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": msg }),
            );
        }
    };
    match queue.retry_job(&job_id, queue_name).await {
        Ok(()) => (
            http::StatusCode::OK,
            serde_json::json!({
                "success": true,
                "message": "Job retried successfully"
            }),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_delete_job(
    queue: &Queue,
    job_id_str: &str,
    queue_name: &str,
) -> (http::StatusCode, serde_json::Value) {
    let job_id = match job_id_from_path(job_id_str) {
        Ok(id) => id,
        Err(msg) => {
            return (
                http::StatusCode::BAD_REQUEST,
                serde_json::json!({ "error": msg }),
            );
        }
    };
    match queue.delete_job(&job_id, queue_name).await {
        Ok(()) => (
            http::StatusCode::OK,
            serde_json::json!({
                "success": true,
                "message": "Job deleted successfully"
            }),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_clean_queue(
    queue: &Queue,
    body: &CleanQueueRequest,
) -> (http::StatusCode, serde_json::Value) {
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
                return (
                    http::StatusCode::BAD_REQUEST,
                    serde_json::json!({
                        "error": "Invalid state. Must be: waiting, delayed, failed, completed, or all"
                    }),
                );
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
                        tracing::error!("Failed to delete job {}: {}", job.id, e);
                    } else {
                        deleted_count += 1;
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to list jobs for cleaning: {}", e);
            }
        }
    }

    (
        http::StatusCode::OK,
        serde_json::json!({
            "success": true,
            "message": format!("Cleaned {} jobs", deleted_count),
            "deleted_count": deleted_count
        }),
    )
}

pub async fn api_recover_stalled(
    queue: &Queue,
    queue_name: &str,
) -> (http::StatusCode, serde_json::Value) {
    let max_stalled_secs = 60u64;
    match queue
        .recover_stalled_jobs(queue_name, max_stalled_secs)
        .await
    {
        Ok(recovered_count) => (
            http::StatusCode::OK,
            serde_json::json!({
                "success": true,
                "message": format!("Recovered {} stalled jobs", recovered_count),
                "recovered_count": recovered_count
            }),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_process_delayed(
    queue: &Queue,
    queue_name: &str,
) -> (http::StatusCode, serde_json::Value) {
    match queue.process_delayed(queue_name).await {
        Ok(moved_count) => (
            http::StatusCode::OK,
            serde_json::json!({
                "success": true,
                "message": format!("Moved {} delayed jobs to waiting", moved_count),
                "moved_count": moved_count
            }),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_queue_paused(
    queue: &Queue,
    queue_name: &str,
) -> (http::StatusCode, serde_json::Value) {
    match queue.is_queue_paused(queue_name).await {
        Ok(paused) => (
            http::StatusCode::OK,
            serde_json::json!({ "paused": paused }),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_pause_queue(
    queue: &Queue,
    queue_name: &str,
) -> (http::StatusCode, serde_json::Value) {
    match queue.pause_queue(queue_name).await {
        Ok(()) => (
            http::StatusCode::OK,
            serde_json::json!({ "success": true, "paused": true }),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_resume_queue(
    queue: &Queue,
    queue_name: &str,
) -> (http::StatusCode, serde_json::Value) {
    match queue.resume_queue(queue_name).await {
        Ok(()) => (
            http::StatusCode::OK,
            serde_json::json!({ "success": true, "paused": false }),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_list_repeats(
    queue: &Queue,
    queue_name: &str,
) -> (http::StatusCode, serde_json::Value) {
    match queue.list_repeats(queue_name).await {
        Ok(repeats) => (
            http::StatusCode::OK,
            serde_json::json!({ "repeats": repeats }),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

#[derive(Deserialize)]
pub struct AddRepeatIntervalRequest {
    pub schedule_id: String,
    pub job_name: String,
    pub payload: serde_json::Value,
    #[serde(default)]
    pub options: JobOptions,
    pub interval_secs: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub catch_up: RepeatCatchUp,
    /// Same semantics as [`Queue::upsert_repeat_interval`].
    pub first_fire_in_secs: Option<u64>,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
pub struct AddRepeatCronRequest {
    pub schedule_id: String,
    pub job_name: String,
    pub cron_expr: String,
    pub payload: serde_json::Value,
    #[serde(default)]
    pub options: JobOptions,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

pub async fn api_add_repeat_interval(
    queue: &Queue,
    queue_name: &str,
    body: AddRepeatIntervalRequest,
) -> (http::StatusCode, serde_json::Value) {
    match queue
        .upsert_repeat_interval(
            queue_name,
            &body.schedule_id,
            &body.job_name,
            body.payload,
            body.options,
            body.interval_secs,
            body.enabled,
            body.catch_up,
            body.first_fire_in_secs,
        )
        .await
    {
        Ok(()) => (
            http::StatusCode::OK,
            serde_json::json!({ "success": true, "schedule_id": body.schedule_id }),
        ),
        Err(e) => (
            http::StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_add_repeat_cron(
    queue: &Queue,
    queue_name: &str,
    body: AddRepeatCronRequest,
) -> (http::StatusCode, serde_json::Value) {
    match queue
        .upsert_repeat_cron(
            queue_name,
            &body.schedule_id,
            &body.job_name,
            &body.cron_expr,
            body.payload,
            body.options,
            body.enabled,
        )
        .await
    {
        Ok(()) => (
            http::StatusCode::OK,
            serde_json::json!({ "success": true, "schedule_id": body.schedule_id }),
        ),
        Err(e) => (
            http::StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_remove_repeat(
    queue: &Queue,
    queue_name: &str,
    schedule_id: &str,
) -> (http::StatusCode, serde_json::Value) {
    match queue.remove_repeat(queue_name, schedule_id).await {
        Ok(()) => (http::StatusCode::OK, serde_json::json!({ "success": true })),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub async fn api_process_repeat(
    queue: &Queue,
    queue_name: &str,
    registry: Option<&JobRegistry>,
) -> (http::StatusCode, serde_json::Value) {
    let Some(reg) = registry else {
        return (
            http::StatusCode::SERVICE_UNAVAILABLE,
            serde_json::json!({
                "error": "WebUIMountConfig.job_registry is not set; cannot process repeat jobs from the dashboard."
            }),
        );
    };
    match queue.process_repeat(queue_name, reg).await {
        Ok(n) => (
            http::StatusCode::OK,
            serde_json::json!({
                "success": true,
                "promoted_count": n,
            }),
        ),
        Err(e) => (
            http::StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": e.to_string() }),
        ),
    }
}

pub fn verify_credentials(auth: &UiLoginRuntime, username: &str, password: &str) -> bool {
    let ok_user = username == auth.expected_username;
    ok_user && bcrypt::verify(password, &auth.password_hash).unwrap_or(false)
}
