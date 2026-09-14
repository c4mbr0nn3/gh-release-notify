#![allow(dead_code)]

mod assets;
mod auth;

use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{middleware, Router};
use tokio::sync::watch;
use tracing::info;

use crate::config::Config;
use crate::config_writeback::ConfigEdit;
use crate::scheduler::StatusSnapshot;

#[allow(unused_imports)]
pub use crate::config::AuthMode;
pub use auth::{LoginLimiter, Sessions};

#[derive(Clone)]
pub struct AppState {
    pub config_rx: watch::Receiver<Arc<Config>>,
    pub config_tx: watch::Sender<Arc<Config>>,
    pub status_rx: watch::Receiver<Arc<StatusSnapshot>>,
    pub config_path: String,
    pub sessions: Arc<Sessions>,
    pub limiter: Arc<LoginLimiter>,
    pub started_at: Instant,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/app.js", get(serve_js))
        .route("/pico.css", get(serve_css))
        .route("/login", post(login))
        .route("/healthz", get(healthz))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/logout", post(logout))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ))
        .with_state(state)
}

pub async fn serve(state: AppState, shutdown: watch::Receiver<bool>) -> Result<()> {
    let addr = {
        let cfg = state.config_rx.borrow().clone();
        format!("{}:{}", cfg.ui.bind_addr, cfg.ui.port)
    };
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow!("failed to bind UI on {addr}: {e}"))?;
    info!("ui listening on http://{addr}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            let mut shutdown = shutdown;
            let _ = shutdown.changed().await;
        })
        .await
        .map_err(|e| anyhow!("ui server error: {e}"))
}

async fn serve_index() -> Response {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

async fn serve_js() -> Response {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

async fn serve_css() -> Response {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

async fn healthz(State(_state): State<AppState>) -> Response {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

async fn login() -> Response {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

async fn get_config(State(_state): State<AppState>) -> Response {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

async fn put_config(
    State(_state): State<AppState>,
    axum::Json(_edit): axum::Json<ConfigEdit>,
) -> Response {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

async fn logout(State(_state): State<AppState>) -> Response {
    StatusCode::NOT_IMPLEMENTED.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::env;
    use tower::ServiceExt;

    fn test_config(token: &str, proxy: bool) -> Config {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        let contents = format!(
            r#"
poll_interval_seconds = 3600
state_path = "./state.json"
sender = "bot@homelab.local"
repos = ["fosrl/pangolin"]
recipients = ["you@example.com"]

[smtp]
host = "smtp.example.com"
port = 587
encryption = "starttls"
username = "postmaster"
password = "secret"

[ui]
admin_token = "{token}"
trust_proxy_auth = {proxy}
"#
        );
        std::fs::write(&p, contents).unwrap();
        Config::load(p.to_str().unwrap()).unwrap()
    }

    async fn test_app(cfg: Config) -> Router {
        let cfg = Arc::new(cfg);
        let (config_tx, config_rx) = watch::channel(cfg);
        let (_status_tx, status_rx) = watch::channel(Arc::new(StatusSnapshot {
            repos: Vec::new(),
            last_poll_finished_at: None,
            next_poll_at: None,
        }));
        let state = AppState {
            config_rx,
            config_tx,
            status_rx,
            config_path: "unused".to_string(),
            sessions: Arc::new(Sessions::default()),
            limiter: Arc::new(LoginLimiter::default()),
            started_at: Instant::now(),
        };
        router(state)
    }

    async fn token_app(token: &str) -> Router {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::remove_var("ADMIN_TOKEN");
        test_app(test_config(token, false)).await
    }

    fn req(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn unauthenticated_request_is_rejected() {
        let app = token_app("secret").await;
        let resp = app.oneshot(req("/api/config")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_token_is_rejected() {
        let app = token_app("secret").await;
        let r = Request::builder()
            .uri("/api/config")
            .header("authorization", "Bearer nope")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn valid_bearer_token_passes_the_auth_layer() {
        let app = token_app("secret").await;
        let r = Request::builder()
            .uri("/api/config")
            .header("authorization", "Bearer secret")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(r).await.unwrap();
        assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn public_routes_need_no_auth() {
        let app = token_app("secret").await;
        for uri in ["/", "/app.js", "/pico.css", "/healthz"] {
            let resp = app.clone().oneshot(req(uri)).await.unwrap();
            assert_ne!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{uri} must be public"
            );
        }
        let r = Request::builder()
            .method("POST")
            .uri("/login")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"admin_token":"secret"}"#))
            .unwrap();
        let resp = app.oneshot(r).await.unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "/login must be public"
        );
    }

    #[tokio::test]
    async fn open_mode_allows_everything() {
        let app = token_app("").await;
        let resp = app.oneshot(req("/api/config")).await.unwrap();
        assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn mutation_without_custom_header_is_rejected() {
        let app = token_app("secret").await;
        let r = Request::builder()
            .method("PUT")
            .uri("/api/config")
            .header("authorization", "Bearer secret")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn proxy_mode_rejects_absent_remote_user() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::remove_var("ADMIN_TOKEN");
        let app = test_app(test_config("", true)).await;
        let resp = app.oneshot(req("/api/config")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
