use anyhow::{bail, Result};
use serde::Deserialize;
use tracing::warn;

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub name: String,
    pub html_url: String,
    #[serde(default)]
    pub body: String,
    pub published_at: chrono::DateTime<chrono::Utc>,
}

pub struct GithubClient {
    client: reqwest::Client,
    token: Option<String>,
}

impl GithubClient {
    pub fn new(token: Option<String>) -> Result<GithubClient> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("pangolin-notify/0.1 (+https://github.com/fosrl)")
            .build()?;
        Ok(GithubClient { client, token })
    }

    pub async fn latest_stable_release(&self, repo: &str) -> Result<Option<Release>> {
        let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
        let mut req = self.client.get(&url);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if let Some(remaining) = resp
            .headers()
            .get("X-RateLimit-Remaining")
            .and_then(|v| v.to_str().ok())
        {
            if let Ok(n) = remaining.parse::<u64>() {
                if n < 10 {
                    warn!("github rate limit low: {n} requests remaining");
                }
            }
        }
        if status == reqwest::StatusCode::FORBIDDEN {
            let body = resp.text().await.unwrap_or_default();
            bail!("github rate-limited (403) for {repo}: {body}");
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!(
                "github request for {repo} failed: status {status}, body: {}",
                body.chars().take(200).collect::<String>()
            );
        }
        let release: Release = resp.json().await?;
        Ok(Some(release))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_json() {
        let raw = r#"{
            "tag_name": "1.19.4",
            "name": "1.19.4",
            "html_url": "https://github.com/fosrl/pangolin/releases/tag/1.19.4",
            "body": "Fix newly created clients from logging in on a new device.",
            "published_at": "2026-06-26T14:29:00Z"
        }"#;
        let r: Release = serde_json::from_str(raw).unwrap();
        assert_eq!(r.tag_name, "1.19.4");
        assert_eq!(r.name, "1.19.4");
        assert_eq!(
            r.html_url,
            "https://github.com/fosrl/pangolin/releases/tag/1.19.4"
        );
        assert!(r.body.contains("Fix newly created clients"));
        assert_eq!(r.published_at.format("%Y-%m-%d").to_string(), "2026-06-26");
    }

    #[test]
    fn parses_release_with_empty_body() {
        let raw = r#"{
            "tag_name": "1.0.0",
            "name": "1.0.0",
            "html_url": "https://example.com",
            "published_at": "2026-01-01T00:00:00Z"
        }"#;
        let r: Release = serde_json::from_str(raw).unwrap();
        assert_eq!(r.body, "");
    }
}
