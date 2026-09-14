#![allow(dead_code)]

mod assets;
mod auth;

use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{middleware, Router};
use serde_json::json;
use tokio::sync::watch;
use tracing::info;

use crate::config::Config;
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
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        assets::INDEX_HTML,
    )
        .into_response()
}

async fn serve_js() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        assets::APP_JS,
    )
        .into_response()
}

async fn serve_css() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        assets::PICO_CSS,
    )
        .into_response()
}

async fn healthz(State(state): State<AppState>) -> Response {
    let status = state.status_rx.borrow().clone();
    axum::Json(json!({
        "status": "ok",
        "uptime_seconds": state.started_at.elapsed().as_secs(),
        "next_poll_at": status.next_poll_at,
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
struct LoginBody {
    admin_token: String,
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<LoginBody>,
) -> Response {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .unwrap_or("unknown")
        .trim()
        .to_string();

    if !state.limiter.allow(&ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            axum::Json(json!({"error": "too many attempts"})),
        )
            .into_response();
    }

    let cfg = state.config_rx.borrow().clone();
    let expected = cfg.admin_token();
    if expected.is_empty() || !auth::constant_time_eq(&body.admin_token, &expected) {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error": "unauthorized"})),
        )
            .into_response();
    }

    state.limiter.reset(&ip);
    let session = state.sessions.create();
    let secure = auth::is_https(&headers);
    (
        StatusCode::OK,
        [(header::SET_COOKIE, auth::cookie_header(&session, secure))],
        axum::Json(json!({"ok": true})),
    )
        .into_response()
}

async fn get_config(State(state): State<AppState>) -> Response {
    let cfg = state.config_rx.borrow().clone();
    let status = state.status_rx.borrow().clone();
    let encryption = serde_json::to_value(cfg.smtp.encryption).unwrap_or_default();
    let auth_mode = match cfg.ui_auth_mode() {
        crate::config::AuthMode::Token => "token",
        crate::config::AuthMode::Proxy => "proxy",
        crate::config::AuthMode::Open => "open",
    };
    let config_writable = probe_config_writable(&state.config_path);
    axum::Json(json!({
        "config": {
            "poll_interval_seconds": cfg.poll_interval_seconds,
            "cron_expression": cfg.cron_expression,
            "sender": cfg.sender,
            "repos": cfg.repos,
            "recipients": cfg.recipients,
            "smtp": {
                "host": cfg.smtp.host,
                "port": cfg.smtp.port,
                "encryption": encryption,
                "username": cfg.smtp.username,
            },
            "ui": {
                "bind_addr": cfg.ui.bind_addr,
                "port": cfg.ui.port,
                "admin_token_set": !cfg.admin_token().is_empty(),
                "trust_proxy_auth": cfg.ui.trust_proxy_auth,
            },
        },
        "readonly": { "state_path": cfg.state_path },
        "env_managed": {
            "smtp_password": std::env::var("SMTP_PASSWORD").is_ok(),
            "github_token": cfg.github_token().is_some(),
        },
        "status": {
            "repos": status.repos,
            "last_poll_finished_at": status.last_poll_finished_at,
            "next_poll_at": status.next_poll_at,
        },
        "config_writable": config_writable,
        "auth_mode": auth_mode,
    }))
    .into_response()
}

fn probe_config_writable(path: &str) -> bool {
    let tmp = format!("{}.tmp", path);
    let opened = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&tmp);
    match opened {
        Ok(_) => {
            let _ = std::fs::remove_file(&tmp);
            true
        }
        Err(_) => false,
    }
}

async fn put_config(
    State(state): State<AppState>,
    axum::Json(edit): axum::Json<crate::config_writeback::ConfigEdit>,
) -> Response {
    let path = state.config_path.clone();
    let result =
        tokio::task::spawn_blocking(move || crate::config_writeback::apply(&path, &edit)).await;

    match result {
        Ok(Ok(cfg)) => {
            let _ = state.config_tx.send(Arc::new(cfg));
            (StatusCode::OK, axum::Json(json!({"ok": true}))).into_response()
        }
        Ok(Err(crate::config_writeback::SaveError::Invalid(m))) => {
            (StatusCode::BAD_REQUEST, axum::Json(json!({"error": m}))).into_response()
        }
        Ok(Err(crate::config_writeback::SaveError::Unwritable(m))) => {
            (StatusCode::CONFLICT, axum::Json(json!({"error": m}))).into_response()
        }
        Ok(Err(crate::config_writeback::SaveError::Io(m))) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({"error": m})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({"error": format!("task join error: {e}")})),
        )
            .into_response(),
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(id) = auth::session_cookie_from_headers(&headers) {
        state.sessions.destroy(&id);
    }
    (
        StatusCode::OK,
        [(header::SET_COOKIE, auth::clear_cookie_header())],
        axum::Json(json!({"ok": true})),
    )
        .into_response()
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

    fn build_app(cfg: Config, path: &str) -> Router {
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
            config_path: path.to_string(),
            sessions: Arc::new(Sessions::default()),
            limiter: Arc::new(LoginLimiter::default()),
            started_at: Instant::now(),
        };
        router(state)
    }

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::remove_var("ADMIN_TOKEN");
        env::remove_var("SMTP_PASSWORD");
        guard
    }

    fn app(token: &str, proxy: bool) -> Router {
        build_app(test_config(token, proxy), "unused")
    }

    fn req(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn unauthenticated_request_is_rejected() {
        let _guard = lock_env();
        let app = app("secret", false);
        let resp = app.oneshot(req("/api/config")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_token_is_rejected() {
        let _guard = lock_env();
        let app = app("secret", false);
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
        let _guard = lock_env();
        let app = app("secret", false);
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
        let _guard = lock_env();
        let app = app("secret", false);
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
    async fn index_and_healthz_return_200() {
        let _guard = lock_env();
        let app = app("secret", false);
        let resp = app.clone().oneshot(req("/")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resp = app.oneshot(req("/healthz")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn open_mode_allows_everything() {
        let _guard = lock_env();
        let app = app("", false);
        let resp = app.oneshot(req("/api/config")).await.unwrap();
        assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn mutation_without_custom_header_is_rejected() {
        let _guard = lock_env();
        let app = app("secret", false);
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
        let _guard = lock_env();
        let app = app("", true);
        let resp = app.oneshot(req("/api/config")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn valid_bearer_token_is_accepted() {
        let _guard = lock_env();
        let app = app("secret", false);
        let r = Request::builder()
            .uri("/api/config")
            .header("authorization", "Bearer secret")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn valid_session_cookie_is_accepted() {
        let _guard = lock_env();
        let app = app("secret", false);
        let r = Request::builder()
            .method("POST")
            .uri("/login")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"admin_token":"secret"}"#))
            .unwrap();
        let resp = app.clone().oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cookie = resp
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(cookie.starts_with("grn_session="));
        let session = cookie.split(';').next().unwrap();
        let r = Request::builder()
            .uri("/api/config")
            .header("cookie", session)
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn proxy_mode_accepts_remote_user() {
        let _guard = lock_env();
        let app = app("", true);
        let r = Request::builder()
            .uri("/api/config")
            .header("remote-user", "operator")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn no_secret_appears_in_config_payload() {
        let _guard = lock_env();
        let app = app("TOP-SECRET-ADMIN-TOKEN", false);
        let r = Request::builder()
            .uri("/api/config")
            .header("authorization", "Bearer TOP-SECRET-ADMIN-TOKEN")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(r).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(
            !body.contains("TOP-SECRET-ADMIN-TOKEN"),
            "admin token leaked: {body}"
        );
        assert!(
            !body.contains("secret"),
            "smtp password literal 'secret' leaked: {body}"
        );
        assert!(body.contains("admin_token_set"), "flag missing: {body}");
        assert!(
            body.contains("true"),
            "admin_token_set should be true: {body}"
        );
    }
}
