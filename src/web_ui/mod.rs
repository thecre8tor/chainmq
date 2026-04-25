//! ChainMQ dashboard: mount routes on **your** Axum or Actix server (no HTTP server inside this crate).
//!
//! - **Axum** (feature `web-ui-axum`, also enabled by default alias `web-ui`): [`chainmq_dashboard_router`].
//! - **Actix** (feature `web-ui-actix`): [`configure_chainmq_web_ui`].

#[cfg(feature = "web-ui-core")]
mod core;

#[cfg(feature = "web-ui-axum")]
mod axum;

#[cfg(feature = "web-ui-actix")]
mod actix;

#[cfg(feature = "web-ui-core")]
pub use core::{
    AddRepeatCronRequest, AddRepeatIntervalRequest, WebUIAuth, WebUIMountConfig,
    embedded_asset_rel_key, embedded_content_type, full_path_for_embedded_request,
    is_ui_auth_public_route, normalize_static_url_prefix, session_cookie_path,
};

#[cfg(feature = "web-ui-axum")]
pub use axum::{WebUiState, chainmq_dashboard_router};

#[cfg(feature = "web-ui-actix")]
pub use actix::configure_chainmq_web_ui;

#[cfg(all(test, feature = "web-ui-core"))]
mod core_tests {
    use super::core::*;

    #[test]
    fn default_mount_config_enables_auth_with_chainmq_credentials() {
        let c = WebUIMountConfig::default();
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

    #[test]
    fn normalize_static_url_prefix_examples() {
        assert_eq!(normalize_static_url_prefix("/"), "/");
        assert_eq!(normalize_static_url_prefix("/dashboard/"), "/dashboard");
        assert_eq!(normalize_static_url_prefix("admin"), "/admin");
    }

    #[test]
    fn full_path_for_embedded_request_nested_axum_paths() {
        assert_eq!(
            full_path_for_embedded_request("/", "/dashboard"),
            "/dashboard/"
        );
        assert_eq!(
            full_path_for_embedded_request("/app.js", "/dashboard"),
            "/dashboard/app.js"
        );
        assert_eq!(
            full_path_for_embedded_request("/dashboard/app.js", "/dashboard"),
            "/dashboard/app.js"
        );
        assert_eq!(full_path_for_embedded_request("/", "/"), "/");
        assert_eq!(
            full_path_for_embedded_request("/styles.css", "/"),
            "/styles.css"
        );
    }

    #[test]
    fn embedded_asset_rel_key_root_and_prefix() {
        assert_eq!(
            embedded_asset_rel_key("/", "/").as_deref(),
            Some("index.html")
        );
        assert_eq!(
            embedded_asset_rel_key("/styles.css", "/").as_deref(),
            Some("styles.css")
        );
        assert_eq!(embedded_asset_rel_key("/api/foo", "/"), None);
        assert_eq!(
            embedded_asset_rel_key("/dashboard", "/dashboard").as_deref(),
            Some("index.html")
        );
        assert_eq!(
            embedded_asset_rel_key("/dashboard/app.js", "/dashboard").as_deref(),
            Some("app.js")
        );
    }

    #[test]
    fn session_secret_accepts_64_bytes_directly() {
        let secret = vec![7u8; 64];
        let cfg = WebUIMountConfig {
            session_secret: Some(secret.clone()),
            ..Default::default()
        };
        let material = session_signing_key_material(&cfg).expect("64-byte secret should work");
        let SessionSecretMaterial::Bytes64(derived) = material else {
            panic!("expected 64-byte material");
        };
        assert_eq!(derived.as_slice(), secret.as_slice());
    }

    #[test]
    fn session_secret_accepts_32_bytes_via_derivation() {
        let cfg = WebUIMountConfig {
            session_secret: Some(vec![9u8; 32]),
            ..Default::default()
        };
        let material = session_signing_key_material(&cfg).expect("32-byte secret should work");
        let SessionSecretMaterial::Bytes32(derived) = material else {
            panic!("expected 32-byte material");
        };
        assert_eq!(derived.len(), 32);
        assert_ne!(derived, [0u8; 32]);
    }

    #[test]
    fn session_secret_rejects_unsupported_lengths() {
        let cfg = WebUIMountConfig {
            session_secret: Some(vec![1u8; 48]),
            ..Default::default()
        };
        let err = session_signing_key_material(&cfg).expect_err("48-byte secret must fail");
        assert!(
            err.to_string()
                .contains("WebUIMountConfig.session_secret must be 32 or 64 bytes")
        );
    }
}

#[cfg(all(test, feature = "web-ui-axum"))]
mod axum_tests {
    use super::chainmq_dashboard_router;
    use super::core::{UiAssets, WebUIMountConfig, embedded_content_type};
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::util::ServiceExt;

    #[tokio::test]
    async fn dashboard_serves_index_when_nested_under_prefix() {
        let options = crate::QueueOptions {
            redis: crate::RedisClient::Url("redis://127.0.0.1:6379".into()),
            ..Default::default()
        };
        let Ok(queue) = crate::Queue::new(options).await else {
            return;
        };
        let inner = chainmq_dashboard_router(
            queue,
            WebUIMountConfig {
                ui_path: "/dashboard".to_string(),
                auth: None,
                ..Default::default()
            },
        )
        .expect("router");
        let app = axum::Router::new().nest_service("/dashboard", inner);

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/dashboard/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(text.contains("ChainMQ Dashboard"));
    }

    #[tokio::test]
    async fn dashboard_serves_index_html_at_root_mount() {
        let options = crate::QueueOptions {
            redis: crate::RedisClient::Url("redis://127.0.0.1:6379".into()),
            ..Default::default()
        };
        let Ok(queue) = crate::Queue::new(options).await else {
            // No Redis in CI
            return;
        };
        let app = chainmq_dashboard_router(
            queue,
            WebUIMountConfig {
                ui_path: "/".to_string(),
                auth: None,
                ..Default::default()
            },
        )
        .expect("router");

        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .expect("response");
        assert_eq!(res.status(), StatusCode::OK);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(text.contains("ChainMQ Dashboard"));
    }

    #[tokio::test]
    async fn api_queues_forbidden_without_sec_fetch_site() {
        let options = crate::QueueOptions {
            redis: crate::RedisClient::Url("redis://127.0.0.1:6379".into()),
            ..Default::default()
        };
        let Ok(queue) = crate::Queue::new(options).await else {
            return;
        };
        let app = chainmq_dashboard_router(
            queue,
            WebUIMountConfig {
                ui_path: "/".to_string(),
                auth: None,
                ..Default::default()
            },
        )
        .expect("router");

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/queues")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn api_queues_ok_with_same_origin_header() {
        let options = crate::QueueOptions {
            redis: crate::RedisClient::Url("redis://127.0.0.1:6379".into()),
            ..Default::default()
        };
        let Ok(queue) = crate::Queue::new(options).await else {
            return;
        };
        let app = chainmq_dashboard_router(
            queue,
            WebUIMountConfig {
                ui_path: "/".to_string(),
                auth: None,
                ..Default::default()
            },
        )
        .expect("router");

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/queues")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("response");
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[test]
    fn embedded_asset_bytes_match_index() {
        let f = UiAssets::get("index.html").expect("index embedded");
        assert!(f.data.len() > 100);
        assert_eq!(
            embedded_content_type("index.html"),
            "text/html; charset=utf-8"
        );
    }
}
