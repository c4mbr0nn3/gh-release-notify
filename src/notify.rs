use anyhow::{anyhow, Result};
use lettre::{
    message::header::ContentType, transport::smtp::authentication::Credentials, AsyncSmtpTransport,
    AsyncTransport, Message, Tokio1Executor,
};
use tracing::info;

use crate::config::{Config, Encryption};
use crate::github::Release;

pub fn build_body(release: &Release, repo: &str) -> String {
    format!(
        "A new stable release of {repo} is available.\n\n\
         Tag: {tag}\n\
         Name: {name}\n\
         Published: {published}\n\
         URL: {url}\n\n\
         Release notes:\n\
         {body}\n",
        repo = repo,
        tag = release.tag_name,
        name = release.name,
        published = release.published_at.format("%Y-%m-%d %H:%M UTC"),
        url = release.html_url,
        body = release.body,
    )
}

pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    sender: String,
}

impl Mailer {
    pub fn new(cfg: &Config) -> Result<Mailer> {
        let mut builder = match cfg.smtp.encryption {
            Encryption::StartTls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp.host)
                    .map_err(|e| anyhow!("failed to build STARTTLS transport: {e}"))?
            }
            Encryption::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.smtp.host)
                .map_err(|e| anyhow!("failed to build TLS transport: {e}"))?,
            Encryption::None => {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.smtp.host)
            }
        };
        builder = builder.port(cfg.smtp.port);
        if !cfg.smtp.username.is_empty() {
            builder = builder.credentials(Credentials::new(
                cfg.smtp.username.clone(),
                cfg.smtp_password(),
            ));
        }
        Ok(Mailer {
            transport: builder.build(),
            sender: cfg.sender.clone(),
        })
    }

    pub async fn send_new_release(
        &self,
        release: &Release,
        repo: &str,
        recipients: &[String],
    ) -> Result<()> {
        let body = build_body(release, repo);
        let subject = format!("[gh-release-notify] {} {} released", repo, release.tag_name);

        let mut builder = Message::builder().from(
            self.sender
                .parse()
                .map_err(|e| anyhow!("invalid sender '{}': {e}", self.sender))?,
        );
        for r in recipients {
            builder = builder.to(r
                .parse()
                .map_err(|e| anyhow!("invalid recipient '{r}': {e}"))?);
        }
        let email = builder
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body)?;
        self.transport
            .send(email)
            .await
            .map_err(|e| anyhow!("smtp send failed: {e}"))?;
        info!(
            "sent notification to {} recipients for {} {}",
            recipients.len(),
            repo,
            release.tag_name
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_release() -> Release {
        Release {
            tag_name: "1.19.4".to_string(),
            name: "1.19.4".to_string(),
            html_url: "https://github.com/fosrl/pangolin/releases/tag/1.19.4".to_string(),
            body: "Fix newly created clients from logging in on a new device.".to_string(),
            published_at: chrono::Utc
                .with_ymd_and_hms(2026, 6, 26, 14, 29, 0)
                .unwrap(),
        }
    }

    #[test]
    fn body_contains_repo_tag_url_and_notes() {
        let r = sample_release();
        let body = build_body(&r, "fosrl/pangolin");
        assert!(body.contains("fosrl/pangolin"));
        assert!(body.contains("1.19.4"));
        assert!(body.contains("https://github.com/fosrl/pangolin/releases/tag/1.19.4"));
        assert!(body.contains("Fix newly created clients"));
        assert!(body.contains("2026-06-26 14:29 UTC"));
    }
}
