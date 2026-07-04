use anyhow::Result;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::github::{GithubClient, GithubError};
use crate::notify::Mailer;
use crate::state::StateStore;

pub async fn run(
    cfg: Config,
    github: GithubClient,
    mut state: StateStore,
    mailer: Mailer,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let interval = Duration::from_secs(cfg.poll_interval_seconds);
    loop {
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
        info!("tick complete, sleeping {}s", cfg.poll_interval_seconds);
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => {
                info!("shutdown signaled, exiting scheduler");
                break;
            }
        }
    }
    Ok(())
}
