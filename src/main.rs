mod config;
mod config_writeback;
mod github;
mod notify;
mod scheduler;
mod state;
mod ui;

#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

use clap::Parser;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "gh-release-notify",
    about = "Email notifier for new GitHub releases"
)]
struct Args {
    #[arg(long, env = "CONFIG_PATH", default_value = "./config.toml")]
    config: String,
    #[arg(long, env = "STATE_PATH")]
    state_path: Option<String>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    info!("loading config from {}", args.config);

    let mut cfg = match config::Config::load(&args.config) {
        Ok(c) => c,
        Err(e) => {
            error!("invalid config: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = cfg.apply_state_path_override(args.state_path.as_deref()) {
        error!("invalid state path override: {e}");
        std::process::exit(1);
    }

    if cfg.cron_schedule.is_some() {
        info!(
            "config loaded: {} repos, cron schedule '{}', state path {}, token {}",
            cfg.repos.len(),
            cfg.cron_expression.as_deref().unwrap_or(""),
            cfg.state_path,
            if cfg.github_token().is_some() {
                "present"
            } else {
                "absent"
            }
        );
    } else {
        info!(
            "config loaded: {} repos, poll interval {}s, state path {}, token {}",
            cfg.repos.len(),
            cfg.poll_interval_seconds,
            cfg.state_path,
            if cfg.github_token().is_some() {
                "present"
            } else {
                "absent"
            }
        );
    }

    let github = match github::GithubClient::new(cfg.github_token()) {
        Ok(c) => c,
        Err(e) => {
            error!("failed to build github client: {e}");
            std::process::exit(1);
        }
    };

    let state = match state::StateStore::load(&cfg.state_path) {
        Ok(s) => s,
        Err(e) => {
            error!("failed to load state: {e}");
            std::process::exit(1);
        }
    };

    let cfg = Arc::new(cfg);
    let (config_tx, config_rx) = tokio::sync::watch::channel(cfg.clone());
    let (status_tx, status_rx) = tokio::sync::watch::channel(Arc::new(scheduler::StatusSnapshot {
        repos: Vec::new(),
        last_poll_finished_at: None,
        next_poll_at: None,
    }));
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    match cfg.ui_auth_mode() {
        ui::AuthMode::Token => info!("ui auth: admin token login enabled"),
        ui::AuthMode::Proxy => info!("ui auth: trusting Remote-User from reverse proxy"),
        ui::AuthMode::Open => {
            if cfg.ui.bind_addr == "0.0.0.0" {
                tracing::warn!(
                    "ui is bound to 0.0.0.0 with NO authentication; \
                     ensure it is only reachable through your reverse proxy"
                );
            } else {
                info!("ui auth: open (no admin_token set)");
            }
        }
    }

    let ui_state = ui::AppState {
        config_rx: config_rx.clone(),
        status_rx,
        config_path: cfg.config_path.clone(),
        config_tx: config_tx.clone(),
        sessions: Arc::new(ui::Sessions::default()),
        limiter: Arc::new(ui::LoginLimiter::default()),
        started_at: std::time::Instant::now(),
    };

    let ui_shutdown = shutdown_rx.clone();
    let ui_handle = tokio::spawn(async move {
        if let Err(e) = ui::serve(ui_state, ui_shutdown).await {
            error!("ui server exited with error: {e}");
        }
    });

    let scheduler_handle = tokio::spawn(async move {
        if let Err(e) = scheduler::run(config_rx, github, state, status_tx, shutdown_rx).await {
            error!("scheduler exited with error: {e}");
        }
    });

    let _config_tx = config_tx;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => info!("received SIGINT"),
        _ = unix_sigterm() => info!("received SIGTERM"),
    }

    let _ = shutdown_tx.send(true);
    let _ = ui_handle.await;
    let _ = scheduler_handle.await;
    info!("shutdown complete");
}

#[cfg(unix)]
async fn unix_sigterm() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut s = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    s.recv().await;
}

#[cfg(not(unix))]
async fn unix_sigterm() {
    std::future::pending::<()>().await;
}
