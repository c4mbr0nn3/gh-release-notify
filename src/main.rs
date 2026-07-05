mod config;
mod github;
mod notify;
mod scheduler;
mod state;

use clap::Parser;
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

    let cfg = match config::Config::load(&args.config) {
        Ok(c) => c,
        Err(e) => {
            error!("invalid config: {e}");
            std::process::exit(1);
        }
    };

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

    let mailer = match notify::Mailer::new(&cfg) {
        Ok(m) => m,
        Err(e) => {
            error!("failed to build mailer: {e}");
            std::process::exit(1);
        }
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let cfg_clone = cfg.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = scheduler::run(cfg_clone, github, state, mailer, shutdown_rx).await {
            error!("scheduler exited with error: {e}");
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => info!("received SIGINT"),
        _ = unix_sigterm() => info!("received SIGTERM"),
    }

    let _ = shutdown_tx.send(true);
    let _ = handle.await;
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
