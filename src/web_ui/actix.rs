//! Mount the dashboard on an existing Actix [`web::ServiceConfig`] (no `HttpServer` in this crate).

use std::sync::Arc;

use actix_session::SessionExt;
use actix_session::config::CookieContentSecurity;
use actix_session::{Session, SessionMiddleware, storage::CookieSessionStore};
use actix_web::{
    Error, HttpRequest, HttpResponse, Result as ActixResult,
    body::BoxBody,
    cookie::{Key, SameSite},
    dev::{ServiceRequest, ServiceResponse},
    http::header::LOCATION,
    middleware::{DefaultHeaders, Next, from_fn},
    web::{self, ServiceConfig},
};

fn map_http_status(s: http::StatusCode) -> actix_web::http::StatusCode {
    actix_web::http::StatusCode::from_u16(s.as_u16())
        .unwrap_or(actix_web::http::StatusCode::INTERNAL_SERVER_ERROR)
}
use tokio::sync::Mutex;

use crate::Queue;

use super::core::{
    self, CleanQueueRequest, DeleteJobRequest, JobLogsQuery, LoginRequest, SESSION_AUTH_KEY,
    UiAssets, WebUIMountConfig, embedded_asset_rel_key, embedded_content_type,
    is_ui_auth_public_route, json_reauth_value, normalize_static_url_prefix, session_cookie_path,
    session_signing_key_material, verify_credentials,
};

#[derive(Clone)]
struct AppState {
    queue: Arc<Mutex<Queue>>,
    auth: Option<Arc<core::UiLoginRuntime>>,
}

/// Register dashboard routes on `cfg`. Call this from `App::configure`.
///
/// Pass a shared [`Arc`]`<`[`Mutex`]`<`[`Queue`]`>>` so every Actix worker uses the same queue handle
/// (for example clone the arc in your `HttpServer::new` closure).
///
/// ```ignore
/// use actix_web::{App, HttpServer};
/// use std::sync::Arc;
/// use tokio::sync::Mutex;
///
/// let queue = Arc::new(Mutex::new(queue));
/// HttpServer::new({
///     let queue = queue.clone();
///     let config = config.clone();
///     move || {
///         App::new().configure(|cfg| {
///             chainmq::configure_chainmq_web_ui(cfg, queue.clone(), config.clone()).unwrap()
///         })
///     }
/// })
/// ```
pub fn configure_chainmq_web_ui(
    cfg: &mut ServiceConfig,
    queue: Arc<Mutex<Queue>>,
    config: WebUIMountConfig,
) -> std::io::Result<()> {
    let auth: Option<Arc<core::UiLoginRuntime>> = match &config.auth {
        Some(a) => Some(core::build_login_runtime(a)?),
        None => None,
    };

    let app_state = AppState { queue, auth };

    let session_key = Key::from(&session_signing_key_material(&config)[..]);
    let cookie_path = session_cookie_path(&config.ui_path);
    let cookie_secure = config.cookie_secure;

    let static_url_prefix = normalize_static_url_prefix(&config.ui_path);
    let api_path = if static_url_prefix == "/" {
        "/api".to_string()
    } else {
        format!("{}/api", static_url_prefix)
    };

    let static_url_prefix_data = static_url_prefix.clone();
    let api_path_for_scope = api_path.clone();

    let no_store =
        DefaultHeaders::new().add(("Cache-Control", "no-store, max-age=0, must-revalidate"));

    cfg.app_data(web::Data::new(app_state));
    cfg.app_data(web::Data::new(static_url_prefix_data.clone()));

    cfg.service(
        web::scope(&api_path_for_scope)
            .wrap(no_store.clone())
            .wrap(from_fn(require_ui_session_login_actix))
            .wrap(from_fn(ui_internal_json_only_actix))
            .wrap({
                let mut session_builder =
                    SessionMiddleware::builder(CookieSessionStore::default(), session_key.clone())
                        .cookie_name("chainmq_ui".into())
                        .cookie_secure(cookie_secure)
                        .cookie_same_site(SameSite::Lax)
                        .cookie_content_security(CookieContentSecurity::Signed);
                if cookie_path != "/" {
                    session_builder = session_builder.cookie_path(cookie_path.clone());
                }
                session_builder.build()
            })
            .route("/auth/session", web::get().to(get_auth_session_actix))
            .route("/auth/login", web::post().to(post_auth_login_actix))
            .route("/auth/logout", web::post().to(post_auth_logout_actix))
            .route("/queues", web::get().to(get_queues_actix))
            .route(
                "/queues/{queue_name}/stats",
                web::get().to(get_queue_stats_actix),
            )
            .route(
                "/queues/{queue_name}/jobs/{state}",
                web::get().to(list_jobs_actix),
            )
            .route("/jobs/{job_id}/logs", web::get().to(get_job_logs_actix))
            .route("/jobs/{job_id}", web::get().to(get_job_actix))
            .route("/jobs/{job_id}/retry", web::post().to(retry_job_actix))
            .route("/jobs/{job_id}/delete", web::delete().to(delete_job_actix))
            .route("/queues/clean", web::post().to(clean_queue_actix))
            .route(
                "/queues/{queue_name}/recover-stalled",
                web::post().to(recover_stalled_actix),
            )
            .route(
                "/queues/{queue_name}/process-delayed",
                web::post().to(process_delayed_actix),
            ),
    );

    if static_url_prefix == "/" {
        cfg.service(
            web::resource("/")
                .wrap(no_store.clone())
                .route(web::get().to(serve_embedded_actix)),
        );
        cfg.service(
            web::resource("/{path:.*}")
                .wrap(no_store)
                .route(web::get().to(serve_embedded_actix)),
        );
    } else {
        cfg.service(
            web::scope(static_url_prefix.as_str())
                .wrap(no_store)
                .service(web::resource("").route(web::get().to(serve_embedded_actix)))
                .service(web::resource("/").route(web::get().to(serve_embedded_actix)))
                .service(web::resource("/{path:.*}").route(web::get().to(serve_embedded_actix))),
        );
    }

    Ok(())
}

async fn ui_internal_json_only_actix(
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

async fn require_ui_session_login_actix(
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

    if is_ui_auth_public_route(method, &path) {
        return Ok(next.call(req).await?.map_into_boxed_body());
    }

    let session = req.get_session();
    let authenticated = session
        .get::<bool>(SESSION_AUTH_KEY)
        .map_err(actix_web::error::ErrorInternalServerError)?
        .unwrap_or(false);

    if !authenticated {
        let (req, _pl) = req.into_parts();
        let res = HttpResponse::build(actix_web::http::StatusCode::UNAUTHORIZED)
            .json(json_reauth_value("Not authenticated"))
            .map_into_boxed_body();
        return Ok(ServiceResponse::new(req, res));
    }

    Ok(next.call(req).await?.map_into_boxed_body())
}

async fn get_auth_session_actix(
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
    Ok(HttpResponse::Ok().json(core::AuthSessionResponse {
        auth_enabled,
        authenticated,
    }))
}

async fn post_auth_login_actix(
    state: web::Data<AppState>,
    session: Session,
    body: web::Json<LoginRequest>,
) -> ActixResult<HttpResponse> {
    let Some(auth) = state.auth.as_ref() else {
        return Ok(HttpResponse::NotFound().json(serde_json::json!({
            "error": "Authentication is not enabled for this server"
        })));
    };

    if !verify_credentials(auth, &body.username, &body.password) {
        return Ok(
            HttpResponse::build(actix_web::http::StatusCode::UNAUTHORIZED)
                .json(json_reauth_value("Invalid username or password")),
        );
    }

    session
        .insert(SESSION_AUTH_KEY, true)
        .map_err(actix_web::error::ErrorInternalServerError)?;
    session.renew();

    Ok(HttpResponse::Ok().json(serde_json::json!({ "success": true })))
}

async fn post_auth_logout_actix(session: Session) -> ActixResult<HttpResponse> {
    session.purge();
    Ok(HttpResponse::Ok().json(serde_json::json!({ "success": true })))
}

async fn get_queues_actix(state: web::Data<AppState>) -> ActixResult<HttpResponse> {
    let q = state.queue.lock().await;
    let (st, json) = core::api_get_queues(&*q).await;
    Ok(HttpResponse::build(map_http_status(st)).json(json))
}

async fn get_queue_stats_actix(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> ActixResult<HttpResponse> {
    let queue_name = path.into_inner();
    let q = state.queue.lock().await;
    let (st, json) = core::api_get_queue_stats(&*q, &queue_name).await;
    Ok(HttpResponse::build(map_http_status(st)).json(json))
}

async fn list_jobs_actix(
    state: web::Data<AppState>,
    path: web::Path<(String, String)>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> ActixResult<HttpResponse> {
    let (queue_name, state_str) = path.into_inner();
    let limit = query
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(100);
    let q = state.queue.lock().await;
    let (st, json) = core::api_list_jobs(&*q, &queue_name, &state_str, limit).await;
    Ok(HttpResponse::build(map_http_status(st)).json(json))
}

async fn get_job_actix(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> ActixResult<HttpResponse> {
    let job_id = path.into_inner();
    let q = state.queue.lock().await;
    let (st, json) = core::api_get_job(&*q, &job_id).await;
    Ok(HttpResponse::build(map_http_status(st)).json(json))
}

async fn get_job_logs_actix(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<JobLogsQuery>,
) -> ActixResult<HttpResponse> {
    let job_id = path.into_inner();
    let limit = query.limit.unwrap_or(200).clamp(1, 500);
    let q = state.queue.lock().await;
    let (st, json) = core::api_get_job_logs(&*q, &job_id, limit).await;
    Ok(HttpResponse::build(map_http_status(st)).json(json))
}

async fn retry_job_actix(
    state: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<core::RetryJobRequest>,
) -> ActixResult<HttpResponse> {
    let job_id = path.into_inner();
    let q = state.queue.lock().await;
    let (st, json) = core::api_retry_job(&*q, &job_id, &body.queue_name).await;
    Ok(HttpResponse::build(map_http_status(st)).json(json))
}

async fn delete_job_actix(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<DeleteJobRequest>,
) -> ActixResult<HttpResponse> {
    let job_id = path.into_inner();
    let q = state.queue.lock().await;
    let (st, json) = core::api_delete_job(&*q, &job_id, &query.queue_name).await;
    Ok(HttpResponse::build(map_http_status(st)).json(json))
}

async fn clean_queue_actix(
    state: web::Data<AppState>,
    body: web::Json<CleanQueueRequest>,
) -> ActixResult<HttpResponse> {
    let q = state.queue.lock().await;
    let (st, json) = core::api_clean_queue(&*q, &body).await;
    Ok(HttpResponse::build(map_http_status(st)).json(json))
}

async fn recover_stalled_actix(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> ActixResult<HttpResponse> {
    let queue_name = path.into_inner();
    let q = state.queue.lock().await;
    let (st, json) = core::api_recover_stalled(&*q, &queue_name).await;
    Ok(HttpResponse::build(map_http_status(st)).json(json))
}

async fn process_delayed_actix(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> ActixResult<HttpResponse> {
    let queue_name = path.into_inner();
    let q = state.queue.lock().await;
    let (st, json) = core::api_process_delayed(&*q, &queue_name).await;
    Ok(HttpResponse::build(map_http_status(st)).json(json))
}

async fn serve_embedded_actix(
    req: HttpRequest,
    prefix: web::Data<String>,
) -> ActixResult<HttpResponse> {
    let prefix_str = prefix.get_ref().as_str();
    if prefix_str != "/" {
        let canonical_prefix = prefix_str.trim_end_matches('/');
        if req.path() == canonical_prefix {
            let mut loc = format!("{}/", canonical_prefix);
            if let Some(q) = req.uri().query() {
                loc.push('?');
                loc.push_str(q);
            }
            return Ok(HttpResponse::Found()
                .insert_header((LOCATION, loc))
                .finish());
        }
    }
    let Some(rel) = embedded_asset_rel_key(req.path(), prefix_str) else {
        return Ok(HttpResponse::NotFound().finish());
    };
    let Some(file) = UiAssets::get(&rel) else {
        return Ok(HttpResponse::NotFound().finish());
    };
    Ok(HttpResponse::Ok()
        .content_type(embedded_content_type(&rel))
        .body(file.data))
}
