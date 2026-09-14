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
