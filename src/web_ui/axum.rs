//! Nestable Axum router for the ChainMQ dashboard (no HTTP server in this crate).

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    body::Body,
    extract::{OriginalUri, Path, Query, State},
    http::{HeaderValue, Request, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post},
};
use cookie::SameSite;
use futures::StreamExt;
use tokio::sync::Mutex;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_sessions::Session;
use tower_sessions_cookie_store::{CookieSessionConfig, CookieSessionManagerLayer, Key};

use crate::Queue;

use super::core::{
    self, CleanQueueRequest, DeleteJobRequest, JobLogsQuery, LoginRequest, QueueEventsQuery,
    RetryJobRequest, SESSION_AUTH_KEY, UiAssets, WebUIMountConfig, embedded_asset_rel_key,
    embedded_content_type, full_path_for_embedded_request, is_ui_auth_public_route,
    json_reauth_value, session_cookie_path, session_signing_key_material, verify_credentials,
};

/// Application state for the dashboard (queue + auth + static URL prefix).
#[derive(Clone)]
pub struct WebUiState {
    pub queue: Arc<Mutex<Queue>>,
    pub auth: Option<Arc<core::UiLoginRuntime>>,
    pub static_prefix: String,
}

/// Build a [`Router`] for the dashboard: routes under `/api/...` plus static files at `/` (relative
/// to where you mount this router).
///
/// Mount on your Axum app with [`Router::nest_service`], using the same path you passed in
/// [`WebUIMountConfig::ui_path`] (after normalizing, e.g. `/dashboard`). Prefer `nest_service`
/// over [`Router::nest`]: Axum’s `nest` does not match `GET /prefix/` when the prefix has no
/// trailing slash, so the dashboard index would 404 at `/dashboard/`.
///
/// ```ignore
/// let dash = chainmq_dashboard_router(queue, config)?;
/// let app = Router::new().nest_service("/dashboard", dash);
/// ```
///
/// If [`WebUIMountConfig::ui_path`] is `"/"`, merge at root instead of nesting:
///
/// ```ignore
/// let app = Router::new().merge(chainmq_dashboard_router(queue, config)?);
/// ```
pub fn chainmq_dashboard_router(queue: Queue, config: WebUIMountConfig) -> std::io::Result<Router> {
    let auth = match &config.auth {
        Some(a) => Some(core::build_login_runtime(a)?),
        None => None,
    };

    let static_prefix = core::normalize_static_url_prefix(&config.ui_path);
    let state = WebUiState {
        queue: Arc::new(Mutex::new(queue)),
        auth,
        static_prefix: static_prefix.clone(),
    };

    let key_bytes = session_signing_key_material(&config);
    let signing_key = Key::from(&key_bytes[..]);

    let cookie_path = session_cookie_path(&config.ui_path);
    let mut cookie_cfg = CookieSessionConfig::default()
        .with_name("chainmq_ui")
        .with_same_site(SameSite::Lax)
        .with_secure(config.cookie_secure);
    if cookie_path != "/" {
        cookie_cfg = cookie_cfg.with_path(cookie_path);
    }

    let session_layer = CookieSessionManagerLayer::signed(signing_key).with_config(cookie_cfg);

    let public = Router::new()
        .route("/auth/session", get(get_auth_session_axum))
        .route("/auth/login", post(post_auth_login_axum))
        .route("/auth/logout", post(post_auth_logout_axum));

    let protected = Router::new()
        .route("/queues", get(get_queues_axum))
        .route("/queues/{queue_name}/stats", get(get_queue_stats_axum))
        .route(
            "/queues/{queue_name}/events/live",
            get(get_queue_events_live_axum),
        )
        .route("/queues/{queue_name}/events", get(get_queue_events_axum))
        .route("/redis/stats", get(get_redis_stats_axum))
        .route("/queues/{queue_name}/jobs/{state}", get(list_jobs_axum))
        .route("/jobs/{job_id}/logs", get(get_job_logs_axum))
        .route("/jobs/{job_id}", get(get_job_axum))
        .route("/jobs/{job_id}/retry", post(retry_job_axum))
        .route("/jobs/{job_id}/delete", delete(delete_job_axum))
        .route("/queues/clean", post(clean_queue_axum))
        .route(
            "/queues/{queue_name}/recover-stalled",
            post(recover_stalled_axum),
        )
        .route(
            "/queues/{queue_name}/process-delayed",
            post(process_delayed_axum),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_ui_session_login_axum,
        ));

    let no_store = SetResponseHeaderLayer::overriding(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0, must-revalidate"),
    );

    let api = Router::new()
        .merge(public)
        .merge(protected)
        .layer(middleware::from_fn(ui_internal_json_only_axum))
        .layer(session_layer)
        .layer(no_store);

    let static_r = Router::new()
        .route("/", get(serve_embedded_axum))
        .route("/{*path}", get(serve_embedded_axum));

    Ok(Router::new()
        .nest("/api", api)
        .merge(static_r)
        .fallback(get(serve_embedded_axum))
        .with_state(state))
}

async fn ui_internal_json_only_axum(
    req: Request<Body>,
    next: Next,
) -> Result<Response, std::convert::Infallible> {
    let site = req
        .headers()
        .get("sec-fetch-site")
        .and_then(|h| h.to_str().ok());
    let allowed = matches!(site, Some("same-origin") | Some("same-site"));
    if allowed {
        Ok(next.run(req).await)
    } else {
        let body = Json(serde_json::json!({
            "error": "This JSON API is internal to the web UI. Open the dashboard in a browser."
        }));
        Ok((StatusCode::FORBIDDEN, body).into_response())
    }
}

async fn require_ui_session_login_axum(
    State(st): State<WebUiState>,
    session: Session,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    if st.auth.is_none() {
        return Ok(next.run(req).await);
    }
    let path = req.uri().path().to_string();
    let method = req.method().as_str();
    if is_ui_auth_public_route(method, &path) {
        return Ok(next.run(req).await);
    }
    let authenticated = session
        .get::<bool>(SESSION_AUTH_KEY)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .unwrap_or(false);
    if !authenticated {
        return Ok((
            StatusCode::UNAUTHORIZED,
            Json(json_reauth_value("Not authenticated")),
        )
            .into_response());
    }
    Ok(next.run(req).await)
}

async fn get_auth_session_axum(
    State(st): State<WebUiState>,
    session: Session,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let auth_enabled = st.auth.is_some();
    let authenticated = if auth_enabled {
        session
            .get::<bool>(SESSION_AUTH_KEY)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .unwrap_or(false)
    } else {
        true
    };
    Ok(Json(
        serde_json::to_value(core::AuthSessionResponse {
            auth_enabled,
            authenticated,
        })
        .unwrap_or_default(),
    ))
}

async fn post_auth_login_axum(
    State(st): State<WebUiState>,
    session: Session,
    Json(body): Json<LoginRequest>,
) -> Result<Response, StatusCode> {
    let Some(auth) = st.auth.as_ref() else {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Authentication is not enabled for this server"
            })),
        )
            .into_response());
    };
    if !verify_credentials(auth, &body.username, &body.password) {
        return Ok((
            StatusCode::UNAUTHORIZED,
            Json(json_reauth_value("Invalid username or password")),
        )
            .into_response());
    }
    session
        .insert(SESSION_AUTH_KEY, true)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    session
        .cycle_id()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "success": true })).into_response())
}

async fn post_auth_logout_axum(session: Session) -> Result<Json<serde_json::Value>, StatusCode> {
    session
        .flush()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "success": true })))
}

async fn get_queues_axum(State(st): State<WebUiState>) -> impl IntoResponse {
    let q = st.queue.lock().await;
    let (status, json) = core::api_get_queues(&*q).await;
    (status, Json(json))
}

async fn get_queue_stats_axum(
    State(st): State<WebUiState>,
    Path(queue_name): Path<String>,
) -> impl IntoResponse {
    let q = st.queue.lock().await;
    let (status, json) = core::api_get_queue_stats(&*q, &queue_name).await;
    (status, Json(json))
}

async fn get_queue_events_axum(
    State(st): State<WebUiState>,
    Path(queue_name): Path<String>,
    Query(qs): Query<QueueEventsQuery>,
) -> impl IntoResponse {
    let limit = qs.limit.unwrap_or(100).clamp(1, 500);
    let q = st.queue.lock().await;
    let (status, json) = core::api_list_queue_events(&*q, &queue_name, limit).await;
    (status, Json(json))
}

async fn get_redis_stats_axum(State(st): State<WebUiState>) -> impl IntoResponse {
    let q = st.queue.lock().await;
    let (status, json) = core::api_get_redis_server_stats(&*q).await;
    (status, Json(json))
}

async fn get_queue_events_live_axum(
    State(st): State<WebUiState>,
    Path(queue_name): Path<String>,
) -> impl IntoResponse {
    let (client_res, channel) = {
        let g = st.queue.lock().await;
        (g.redis_pubsub_client(), g.redis_events_channel(&queue_name))
    };
    let Ok(client) = client_res else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "Live queue events require Redis URL or Client (not ConnectionManager). Use GET …/events for history."
            })),
        )
            .into_response();
    };
    let mut pubsub = match client.get_async_pubsub().await {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("redis pubsub: {e}") })),
            )
                .into_response();
        }
    };
    if let Err(e) = pubsub.subscribe(&channel).await {
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("redis subscribe: {e}") })),
        )
            .into_response();
    }
    let stream = pubsub.into_on_message().map(|msg| {
        let payload: String = msg.get_payload().unwrap_or_default();
        Ok::<Event, Infallible>(Event::default().data(payload))
    });
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(25))
                .text("keepalive"),
        )
        .into_response()
}

async fn list_jobs_axum(
    State(st): State<WebUiState>,
    Path((queue_name, state)): Path<(String, String)>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let limit = query
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(100);
    let q = st.queue.lock().await;
    let (status, json) = core::api_list_jobs(&*q, &queue_name, &state, limit).await;
    (status, Json(json))
}

async fn get_job_axum(
    State(st): State<WebUiState>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let q = st.queue.lock().await;
    let (status, json) = core::api_get_job(&*q, &job_id).await;
    (status, Json(json))
}

async fn get_job_logs_axum(
    State(st): State<WebUiState>,
    Path(job_id): Path<String>,
    Query(qs): Query<JobLogsQuery>,
) -> impl IntoResponse {
    let limit = qs.limit.unwrap_or(200).clamp(1, 500);
    let q = st.queue.lock().await;
    let (status, json) = core::api_get_job_logs(&*q, &job_id, limit).await;
    (status, Json(json))
}

async fn retry_job_axum(
    State(st): State<WebUiState>,
    Path(job_id): Path<String>,
    Json(body): Json<RetryJobRequest>,
) -> impl IntoResponse {
    let q = st.queue.lock().await;
    let (status, json) = core::api_retry_job(&*q, &job_id, &body.queue_name).await;
    (status, Json(json))
}

async fn delete_job_axum(
    State(st): State<WebUiState>,
    Path(job_id): Path<String>,
    Query(del): Query<DeleteJobRequest>,
) -> impl IntoResponse {
    let guard = st.queue.lock().await;
    let (status, json) = core::api_delete_job(&*guard, &job_id, &del.queue_name).await;
    (status, Json(json))
}

async fn clean_queue_axum(
    State(st): State<WebUiState>,
    Json(body): Json<CleanQueueRequest>,
) -> impl IntoResponse {
    let q = st.queue.lock().await;
    let (status, json) = core::api_clean_queue(&*q, &body).await;
    (status, Json(json))
}

async fn recover_stalled_axum(
    State(st): State<WebUiState>,
    Path(queue_name): Path<String>,
) -> impl IntoResponse {
    let q = st.queue.lock().await;
    let (status, json) = core::api_recover_stalled(&*q, &queue_name).await;
    (status, Json(json))
}

async fn process_delayed_axum(
    State(st): State<WebUiState>,
    Path(queue_name): Path<String>,
) -> impl IntoResponse {
    let q = st.queue.lock().await;
    let (status, json) = core::api_process_delayed(&*q, &queue_name).await;
    (status, Json(json))
}

async fn serve_embedded_axum(
    OriginalUri(uri): OriginalUri,
    State(st): State<WebUiState>,
) -> impl IntoResponse {
    let prefix_str = st.static_prefix.as_str();
    let full_path = full_path_for_embedded_request(uri.path(), prefix_str);
    if prefix_str != "/" {
        let canonical_prefix = prefix_str.trim_end_matches('/');
        if full_path == canonical_prefix {
            let mut loc = format!("{}/", canonical_prefix);
            if let Some(q) = uri.query() {
                loc.push('?');
                loc.push_str(q);
            }
            return (
                StatusCode::FOUND,
                [(header::LOCATION, loc.parse::<HeaderValue>().unwrap())],
            )
                .into_response();
        }
    }
    let Some(rel) = embedded_asset_rel_key(&full_path, prefix_str) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(file) = UiAssets::get(&rel) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let ct = embedded_content_type(&rel).parse::<HeaderValue>().unwrap();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, ct)],
        file.data.to_vec(),
    )
        .into_response()
}
