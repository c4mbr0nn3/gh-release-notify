use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{error, info, warn};

use chrono::{DateTime, Utc};

use crate::config::Config;
use crate::github::{GithubClient, GithubError};
use crate::notify::Mailer;
use crate::state::StateStore;

#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoStatus {
    pub repo: String,
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StatusSnapshot {
    pub repos: Vec<RepoStatus>,
    pub last_poll_finished_at: Option<DateTime<Utc>>,
    pub next_poll_at: Option<DateTime<Utc>>,
}

pub async fn run(
    mut config_rx: watch::Receiver<Arc<Config>>,
    github: GithubClient,
    mut state: StateStore,
    status_tx: watch::Sender<Arc<StatusSnapshot>>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        let cfg = config_rx.borrow().clone();
        let interval = Duration::from_secs(cfg.poll_interval_seconds);
        info!("polling {} repos", cfg.repos.len());
        let mut rate_limited = false;
        for repo in &cfg.repos {
            match github.latest_stable_release(repo).await {
                Ok(None) => info!("no release found for {repo}"),
                Ok(Some(release)) => {
                    let current = state.last_seen(repo).map(|s| s.to_string());
                    if current.as_deref() == Some(release.tag_name.as_str()) {
                        info!("no change for {repo} (still {})", release.tag_name);
                    } else if current.is_none() {
                        info!(
                            "first run for {repo}, storing {} without notifying",
                            release.tag_name
                        );
                        state.set(repo, &release.tag_name);
                        if let Err(e) = state.save() {
                            error!("failed to save state after first run for {repo}: {e}");
                        }
                    } else {
                        info!("new release detected for {repo}: {}", release.tag_name);
                        let mailer = match Mailer::new(&cfg) {
                            Ok(m) => m,
                            Err(e) => {
                                error!("failed to build mailer for {repo}: {e}");
                                continue;
                            }
                        };
                        match mailer
                            .send_new_release(&release, repo, &cfg.recipients)
                            .await
                        {
                            Ok(()) => {
                                state.set(repo, &release.tag_name);
                                if let Err(e) = state.save() {
                                    error!("failed to save state after notifying {repo}: {e}");
                                }
                            }
                            Err(e) => error!("failed to notify {repo} {}: {e}", release.tag_name),
                        }
                    }
                }
                Err(e) if matches!(e, GithubError::RateLimited { .. }) => {
                    error!("github rate-limited, skipping remaining repos this tick: {e}");
                    rate_limited = true;
                    break;
                }
                Err(e) => warn!("failed to fetch latest release for {repo}: {e}"),
            }
        }
        if rate_limited {
            error!("rate-limited by github, skipping remaining repos this tick");
        }

        let now = Utc::now();
        let next_poll_at = match &cfg.cron_schedule {
            Some(schedule) => schedule.after(&now).next(),
            None => Some(now + chrono::Duration::seconds(cfg.poll_interval_seconds as i64)),
        };
        let snapshot = StatusSnapshot {
            repos: cfg
                .repos
                .iter()
                .map(|r| RepoStatus {
                    repo: r.clone(),
                    last_seen: state.last_seen(r).map(|s| s.to_string()),
                })
                .collect(),
            last_poll_finished_at: Some(now),
            next_poll_at,
        };
        status_tx.send_replace(Arc::new(snapshot));

        let duration = match next_poll_at {
            Some(next_dt) => {
                info!("tick complete, next poll at {next_dt}");
                (next_dt - now).to_std().unwrap_or_default()
            }
            None => {
                error!(
                    "cron schedule has no future occurrences, \
                     falling back to poll_interval_seconds"
                );
                interval
            }
        };

        tokio::select! {
            _ = tokio::time::sleep(duration) => {}
            _ = config_rx.changed() => {
                info!("config changed, recomputing next poll");
            }
            _ = shutdown.changed() => {
                info!("shutdown signaled, exiting scheduler");
                break;
            }
        }
    }
    Ok(())
}
