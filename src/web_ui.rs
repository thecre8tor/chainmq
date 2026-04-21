// src/web_ui.rs
//! Web UI server for ChainMQ dashboard
#[cfg(feature = "web-ui")]
use crate::{JobState, Queue};
#[cfg(feature = "web-ui")]
use actix_files;
#[cfg(feature = "web-ui")]
use actix_session::config::CookieContentSecurity;
#[cfg(feature = "web-ui")]
use actix_session::{Session, SessionMiddleware, storage::CookieSessionStore};
#[cfg(feature = "web-ui")]
use actix_session::SessionExt;
#[cfg(feature = "web-ui")]
use actix_web::{
    App, Error, HttpResponse, HttpServer, Result as ActixResult,
    body::BoxBody,
    cookie::{Key, SameSite},
    dev::{ServiceRequest, ServiceResponse},
    middleware::{DefaultHeaders, Next, from_fn},
    web,
};
#[cfg(feature = "web-ui")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "web-ui")]
use std::sync::Arc;
#[cfg(feature = "web-ui")]
use tokio::sync::Mutex;

/// Session payload key set after successful dashboard login.
const SESSION_AUTH_KEY: &str = "chainmq_ui_authenticated";

/// Fixed 64-byte development signing key when [`WebUIConfig::session_secret`] is `None`.
/// **Do not use in production** when the dashboard is reachable beyond localhost.
const DEFAULT_WEB_UI_SESSION_SECRET: &[u8; 64] =
    b"chainmq-web-ui-DEV-SESSION-KEY-64B-DO-NOT-USE-IN-PRODUCTION!!!!!";

#[derive(Clone)]
struct UiLoginRuntime {
    expected_username: String,
    password_hash: String,
}

#[derive(Clone)]
struct AppState {
    queue: Arc<Mutex<Queue>>,
    auth: Option<Arc<UiLoginRuntime>>,
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
struct CleanQueueRequest {
    queue_name: String,
    state: String,
}

#[derive(Deserialize)]
struct JobLogsQuery {
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct AuthSessionResponse {
    auth_enabled: bool,
    authenticated: bool,
}

fn json_reauth(status: actix_web::http::StatusCode, message: &str) -> HttpResponse {
    HttpResponse::build(status).json(serde_json::json!({
        "error": message,
        "reauth": true,
    }))
}

// API: Session / auth bootstrap for the SPA
async fn get_auth_session(
    state: web::Data<AppState>,
    session: Session,
) -> ActixResult<HttpResponse> {
    let auth_enabled = state.auth.is_some();
    let authenticated = if auth_enabled {
        session
            .get::<bool>(SESSION_AUTH_KEY)
            .map_err(actix_web::error::ErrorInternalServerError)?
            .unwrap_or(false)
    } else {
        true
    };
    Ok(HttpResponse::Ok().json(AuthSessionResponse {
        auth_enabled,
        authenticated,
    }))
}

async fn post_auth_login(
    state: web::Data<AppState>,
    session: Session,
    body: web::Json<LoginRequest>,
) -> ActixResult<HttpResponse> {
    let Some(auth) = state.auth.as_ref() else {
        return Ok(HttpResponse::NotFound().json(serde_json::json!({
            "error": "Authentication is not enabled for this server"
        })));
    };

    let ok_user = body.username == auth.expected_username;
    let ok_pass = ok_user
        && bcrypt::verify(&body.password, &auth.password_hash).unwrap_or(false);

    if !ok_pass {
        return Ok(json_reauth(
            actix_web::http::StatusCode::UNAUTHORIZED,
            "Invalid username or password",
        ));
    }

    // Do not call `session.clear()` here: with cookie-backed sessions it can prevent the
    // authenticated state from being committed so the next request still appears anonymous.
    session
        .insert(SESSION_AUTH_KEY, true)
        .map_err(actix_web::error::ErrorInternalServerError)?;
    session.renew();

    Ok(HttpResponse::Ok().json(serde_json::json!({ "success": true })))
}

async fn post_auth_logout(session: Session) -> ActixResult<HttpResponse> {
    session.purge();
    Ok(HttpResponse::Ok().json(serde_json::json!({ "success": true })))
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

// API: Job logs (Redis; populated when workers register `job_logs_layer` on their tracing subscriber)
async fn get_job_logs(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<JobLogsQuery>,
) -> ActixResult<HttpResponse> {
    let job_id_str = path.into_inner();
    let job_id = match job_id_str.parse::<uuid::Uuid>() {
        Ok(uuid) => crate::JobId(uuid),
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

/// Static files (`index.html`, `app.js`, `styles.css`, `favicon.svg`, …) are served from this path
/// relative to the process working directory. Not configurable via the public API.
const UI_STATIC_DIR: &str = "./ui";

/// Reject non-browser callers (curl, scripts, other sites): the JSON under `/api` exists only
/// for the bundled SPA. Browsers set `Sec-Fetch-Site: same-origin` (or `same-site`) on fetches
/// from the dashboard.
#[cfg(feature = "web-ui")]
async fn ui_internal_json_only(
    req: ServiceRequest,
    next: Next<BoxBody>,
) -> Result<ServiceResponse<BoxBody>, Error> {
    let site = req
        .headers()
        .get("sec-fetch-site")
        .and_then(|h| h.to_str().ok());
    let allowed = matches!(site, Some("same-origin") | Some("same-site"));
    if allowed {
        Ok(next.call(req).await?.map_into_boxed_body())
    } else {
        let (req, _pl) = req.into_parts();
        let res = HttpResponse::Forbidden()
            .json(serde_json::json!({
                "error": "This JSON API is internal to the web UI. Open the dashboard in a browser."
            }))
            .map_into_boxed_body();
        Ok(ServiceResponse::new(req, res))
    }
}

#[cfg(feature = "web-ui")]
fn is_ui_auth_public_route(method: &str, path: &str) -> bool {
    (method == "GET" && path.ends_with("/auth/session"))
        || (method == "POST" && path.ends_with("/auth/login"))
        || (method == "POST" && path.ends_with("/auth/logout"))
}

/// When dashboard auth is enabled, require a valid session for all `/api` routes except
/// `/auth/login`, `/auth/logout`, and `/auth/session`.
#[cfg(feature = "web-ui")]
async fn require_ui_session_login(
    req: ServiceRequest,
    next: Next<BoxBody>,
) -> Result<ServiceResponse<BoxBody>, Error> {
    let state = req
        .app_data::<web::Data<AppState>>()
        .map(|d| d.get_ref().clone());

    let Some(state) = state else {
        return Err(actix_web::error::ErrorInternalServerError(
            "missing AppState",
        ));
    };

    if state.auth.is_none() {
        return Ok(next.call(req).await?.map_into_boxed_body());
    }

    let path = req.path().to_string();
    let method = req.method().as_str();

    let public = is_ui_auth_public_route(method, &path);

    if public {
        return Ok(next.call(req).await?.map_into_boxed_body());
    }

    let session = req.get_session();
    let authenticated = session
        .get::<bool>(SESSION_AUTH_KEY)
        .map_err(actix_web::error::ErrorInternalServerError)?
        .unwrap_or(false);

    if !authenticated {
        let (req, _pl) = req.into_parts();
        let res = json_reauth(
            actix_web::http::StatusCode::UNAUTHORIZED,
            "Not authenticated",
        )
        .map_into_boxed_body();
        return Ok(ServiceResponse::new(req, res));
    }

    Ok(next.call(req).await?.map_into_boxed_body())
}

#[cfg(feature = "web-ui")]
fn bind_addr_for_listen(host: &str, port: u16) -> String {
    use std::net::IpAddr;
    let host = host.trim();
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(addr)) => format!("[{}]:{}", addr, port),
        _ => format!("{host}:{port}"),
    }
}

#[cfg(feature = "web-ui")]
fn http_origin(host: &str, port: u16) -> String {
    use std::net::IpAddr;
    let host = host.trim();
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(addr)) => format!("http://[{}]:{}", addr, port),
        _ => format!("http://{host}:{port}"),
    }
}

/// Username and password for the built-in dashboard login (session cookie after `POST …/auth/login`).
///
/// Default credentials are **`ChainMQ` / `ChainMQ`** (see [`Default`]). Override in production.
#[cfg(feature = "web-ui")]
#[derive(Clone)]
pub struct WebUIAuth {
    pub username: String,
    pub password: String,
}

#[cfg(feature = "web-ui")]
impl Default for WebUIAuth {
    fn default() -> Self {
        Self {
            username: "ChainMQ".to_string(),
            password: "ChainMQ".to_string(),
        }
    }
}

/// Configuration for the web UI server
#[cfg(feature = "web-ui")]
#[derive(Clone)]
pub struct WebUIConfig {
    /// Host or IP to bind (default: `127.0.0.1`). Use `0.0.0.0` to listen on all IPv4 interfaces
    /// when clients reach the process directly (protect with a firewall or reverse proxy).
    pub bind_host: String,
    /// Port to bind the server to
    pub port: u16,
    /// Base path for the UI (e.g., "/dashboard", "/admin/queues")
    /// Dashboard data is fetched from `{ui_path}/api/...`; those routes are internal to the UI
    /// (same-origin browser requests only), not a supported public HTTP API.
    pub ui_path: String,
    /// When `Some`, the dashboard requires login (HttpOnly session cookie).
    /// The default [`WebUIConfig`] uses `Some(`[`WebUIAuth::default`]`)` (username and password **`ChainMQ`**).
    /// Set to `None` to disable the login screen entirely.
    pub auth: Option<WebUIAuth>,
    /// Master key (at least 64 bytes) used to sign or encrypt the session cookie.
    /// If `None` while [`Self::auth`] is `Some`, a **fixed development key** is used; set this in production.
    pub session_secret: Option<[u8; 64]>,
    /// `Secure` flag on the session cookie (use `true` behind HTTPS).
    pub cookie_secure: bool,
}

#[cfg(feature = "web-ui")]
impl Default for WebUIConfig {
    fn default() -> Self {
        Self {
            bind_host: "127.0.0.1".to_string(),
            port: 8085,
            ui_path: "/".to_string(),
            auth: Some(WebUIAuth::default()),
            session_secret: None,
            cookie_secure: false,
        }
    }
}

#[cfg(feature = "web-ui")]
fn session_signing_key(config: &WebUIConfig) -> std::io::Result<Key> {
    let bytes: &[u8] = match &config.session_secret {
        Some(k) => k.as_slice(),
        None => {
            if config.auth.is_some() {
                tracing::warn!(
                    "[web-ui] Using built-in development session signing key; set WebUIConfig.session_secret in production"
                );
            }
            DEFAULT_WEB_UI_SESSION_SECRET.as_slice()
        }
    };
    Ok(Key::from(bytes))
}

#[cfg(feature = "web-ui")]
fn session_cookie_path(ui_path: &str) -> String {
    let p = ui_path.trim();
    if p.is_empty() || p == "/" {
        "/".to_string()
    } else {
        p.trim_end_matches('/').to_string()
    }
}

/// Start the web UI server with default settings (see [`WebUIConfig::default`]).
///
/// By default the dashboard **requires login** (credentials `ChainMQ` / `ChainMQ` until you change
/// [`WebUIConfig::auth`]). Pass `WebUIConfig { auth: None, .. }` if you want no authentication.
///
/// This function starts the server in the background and blocks until Ctrl+C is pressed.
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
    let auth: Option<Arc<UiLoginRuntime>> = if let Some(a) = &config.auth {
        let hash = bcrypt::hash(&a.password, bcrypt::DEFAULT_COST).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("failed to hash dashboard password: {e}"),
            )
        })?;
        Some(Arc::new(UiLoginRuntime {
            expected_username: a.username.clone(),
            password_hash: hash,
        }))
    } else {
        None
    };

    let auth_enabled = auth.is_some();
    let app_state = AppState {
        queue: Arc::new(Mutex::new(queue)),
        auth,
    };

    let session_key = session_signing_key(&config)?;
    let cookie_path = session_cookie_path(&config.ui_path);
    let cookie_secure = config.cookie_secure;

    let ui_path = config.ui_path.clone();
    let ui_dir = UI_STATIC_DIR.to_string();
    let port = config.port;
    let bind_host = config.bind_host.clone();
    let bind_addr = bind_addr_for_listen(&bind_host, port);
    let origin = http_origin(&bind_host, port);

    let api_path = if ui_path == "/" {
        "/api".to_string()
    } else {
        format!("{}/api", ui_path)
    };

    // Clone for use in closure and println
    let ui_path_for_closure = ui_path.clone();
    let ui_dir_for_closure = ui_dir.clone();
    let api_path_for_closure = api_path.clone();
    let origin_for_print = origin.clone();

    let server = HttpServer::new(move || {
        let mut session_builder = SessionMiddleware::builder(
            CookieSessionStore::default(),
            session_key.clone(),
        )
        .cookie_name("chainmq_ui".into())
        .cookie_secure(cookie_secure)
        .cookie_same_site(SameSite::Lax)
        // Signed cookies are enough for this store (state lives in the cookie); avoids edge cases
        // with encrypted private payloads across browsers.
        .cookie_content_security(CookieContentSecurity::Signed);

        if cookie_path != "/" {
            session_builder = session_builder.cookie_path(cookie_path.clone());
        }

        let session_middleware = session_builder.build();

        let api_scope = api_path_for_closure.clone();
        App::new()
            .wrap(
                DefaultHeaders::new()
                    .add(("Cache-Control", "no-store, max-age=0, must-revalidate")),
            )
            .app_data(web::Data::new(app_state.clone()))
            .service(
                web::scope(&api_scope)
                    // Session must be registered last so it is the outermost layer: it loads the
                    // cookie into the request before `require_ui_session_login` reads `Session`
                    // (that middleware runs inside session, not before it).
                    .wrap(from_fn(require_ui_session_login))
                    .wrap(from_fn(ui_internal_json_only))
                    .wrap(session_middleware)
                    .route("/auth/session", web::get().to(get_auth_session))
                    .route("/auth/login", web::post().to(post_auth_login))
                    .route("/auth/logout", web::post().to(post_auth_logout))
                    .route("/queues", web::get().to(get_queues))
                    .route("/queues/{queue_name}/stats", web::get().to(get_queue_stats))
                    .route(
                        "/queues/{queue_name}/jobs/{state}",
                        web::get().to(list_jobs),
                    )
                    .route("/jobs/{job_id}/logs", web::get().to(get_job_logs))
                    .route("/jobs/{job_id}", web::get().to(get_job))
                    .route("/jobs/{job_id}/retry", web::post().to(retry_job))
                    .route("/jobs/{job_id}/delete", web::delete().to(delete_job))
                    .route("/queues/clean", web::post().to(clean_queue))
                    .route(
                        "/queues/{queue_name}/recover-stalled",
                        web::post().to(recover_stalled),
                    )
                    .route(
                        "/queues/{queue_name}/process-delayed",
                        web::post().to(process_delayed),
                    ),
            )
            .service(
                actix_files::Files::new(&ui_path_for_closure, &ui_dir_for_closure)
                    .index_file("index.html")
                    .prefer_utf8(true),
            )
    })
    .bind(&bind_addr)?
    .run();

    println!(
        "[web-ui] ChainMQ Web UI listening on {} (UI: {}{})",
        bind_addr, origin_for_print, ui_path
    );
    println!(
        "[web-ui] Open {}{} in your browser to view the dashboard",
        origin_for_print, ui_path
    );
    println!(
        "[web-ui] Static files: {} (relative to process CWD — use repo root so UI edits are picked up)",
        UI_STATIC_DIR
    );
    if auth_enabled {
        println!(
            "[web-ui] Dashboard login is enabled — default credentials are ChainMQ / ChainMQ unless you set WebUIConfig.auth; use a strong session_secret in production."
        );
    }
    println!("[web-ui] Press Ctrl+C to stop the server");

    Ok(server)
}

#[cfg(all(test, feature = "web-ui"))]
mod web_ui_tests {
    use super::*;
    use actix_web::body::MessageBody;
    use actix_web::http::header::{HeaderName, HeaderValue, SET_COOKIE};
    use actix_web::http::StatusCode;
    use actix_web::test as actix_test;
    use actix_web::App;

    #[test]
    fn default_web_ui_config_enables_auth_with_chainmq_credentials() {
        let c = WebUIConfig::default();
        assert!(c.auth.is_some());
        let a = c.auth.expect("auth");
        assert_eq!(a.username, "ChainMQ");
        assert_eq!(a.password, "ChainMQ");
    }

    #[test]
    fn is_ui_auth_public_route_examples() {
        assert!(is_ui_auth_public_route("GET", "/api/auth/session"));
        assert!(is_ui_auth_public_route("POST", "/api/auth/login"));
        assert!(is_ui_auth_public_route(
            "POST",
            "/dashboard/api/auth/logout"
        ));
        assert!(!is_ui_auth_public_route("GET", "/api/queues"));
    }

    fn sec_fetch_same_origin() -> (HeaderName, HeaderValue) {
        (
            HeaderName::from_static("sec-fetch-site"),
            HeaderValue::from_static("same-origin"),
        )
    }

    fn cookie_header_from_response<B: MessageBody>(
        resp: &actix_web::dev::ServiceResponse<B>,
    ) -> Option<String> {
        let mut parts = Vec::new();
        for h in resp.headers().get_all(SET_COOKIE) {
            let s = h.to_str().ok()?;
            let pair = s.split(';').next()?.trim();
            if !pair.is_empty() {
                parts.push(pair);
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("; "))
        }
    }

    #[tokio::test]
    #[ignore = "requires Redis at QueueOptions::default redis_url"]
    async fn queues_401_without_session_then_ok_after_login() {
        let queue = crate::Queue::new(crate::QueueOptions::default())
            .await
            .expect("redis");
        let hash = bcrypt::hash("secretpw", 4).expect("bcrypt");
        let app_state = AppState {
            queue: Arc::new(Mutex::new(queue)),
            auth: Some(Arc::new(UiLoginRuntime {
                expected_username: "tuser".into(),
                password_hash: hash,
            })),
        };
        let session_key = Key::from(&[9u8; 64]);
        let app = actix_test::init_service(
            App::new()
                .app_data(web::Data::new(app_state))
                .service(
                    web::scope("/api")
                        .wrap(from_fn(require_ui_session_login))
                        .wrap(from_fn(ui_internal_json_only))
                        .wrap(
                            SessionMiddleware::builder(
                                CookieSessionStore::default(),
                                session_key,
                            )
                            .cookie_name("chainmq_ui".into())
                            .cookie_secure(false)
                            .cookie_content_security(CookieContentSecurity::Signed)
                            .build(),
                        )
                        .route("/auth/session", web::get().to(get_auth_session))
                        .route("/auth/login", web::post().to(post_auth_login))
                        .route("/auth/logout", web::post().to(post_auth_logout))
                        .route("/queues", web::get().to(get_queues)),
                ),
        )
        .await;

        let req = actix_test::TestRequest::get()
            .uri("/api/queues")
            .insert_header(sec_fetch_same_origin())
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let login_req = actix_test::TestRequest::post()
            .uri("/api/auth/login")
            .insert_header(sec_fetch_same_origin())
            .insert_header((
                actix_web::http::header::CONTENT_TYPE,
                "application/json",
            ))
            .set_payload(
                serde_json::json!({"username": "tuser", "password": "secretpw"}).to_string(),
            )
            .to_request();
        let resp = actix_test::call_service(&app, login_req).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let cookie_hdr = cookie_header_from_response(&resp).expect("session cookie");

        let req = actix_test::TestRequest::get()
            .uri("/api/queues")
            .insert_header(sec_fetch_same_origin())
            .insert_header((actix_web::http::header::COOKIE, cookie_hdr))
            .to_request();
        let resp = actix_test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
