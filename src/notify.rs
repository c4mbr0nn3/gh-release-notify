use anyhow::{anyhow, Result};
use lettre::{
    message::{header::ContentType, Mailbox},
    transport::smtp::authentication::Credentials,
    Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
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
        name = release.name.as_deref().unwrap_or(&release.tag_name),
        published = release.published_at.format("%Y-%m-%d %H:%M UTC"),
        url = release.html_url,
        body = release.body,
    )
}

pub fn build_sender(sender: &str, name: Option<&str>) -> Result<Mailbox> {
    let address: Address = sender
        .parse()
        .map_err(|e| anyhow!("invalid sender '{sender}': {e}"))?;
    Ok(Mailbox::new(name.map(str::to_string), address))
}

pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
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
            from: build_sender(&cfg.sender, cfg.sender_name.as_deref())?,
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

        let mut builder = Message::builder().from(self.from.clone());
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
            name: Some("1.19.4".to_string()),
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

    #[test]
    fn body_falls_back_to_tag_when_name_is_null() {
        let mut r = sample_release();
        r.name = None;
        let body = build_body(&r, "vrana/adminer");
        assert!(body.contains("Tag: 1.19.4"));
        assert!(body.contains("Name: 1.19.4"));
    }

    #[test]
    fn sender_mailbox_includes_display_name() {
        let m = build_sender("bot@homelab.local", Some("gh-release-notify")).unwrap();
        assert_eq!(m.name.as_deref(), Some("gh-release-notify"));
        assert_eq!(m.email.to_string(), "bot@homelab.local");
    }

    #[test]
    fn sender_mailbox_without_name_has_none() {
        let m = build_sender("bot@homelab.local", None).unwrap();
        assert_eq!(m.name, None);
        assert_eq!(m.email.to_string(), "bot@homelab.local");
    }

    #[test]
    fn sender_mailbox_accepts_non_ascii_name() {
        let m = build_sender("bot@homelab.local", Some("Überwacher")).unwrap();
        assert_eq!(m.name.as_deref(), Some("Überwacher"));
    }

    #[test]
    fn build_sender_rejects_regex_valid_but_lettre_invalid() {
        let long_local = format!("{}@example.com", "a".repeat(100));
        let err = build_sender(&long_local, None).unwrap_err();
        assert!(err.to_string().contains("invalid sender"));
    }

    fn rendered_from_header(mailbox: Mailbox) -> String {
        let to: Mailbox = "you@example.com".parse().unwrap();
        let email = Message::builder()
            .from(mailbox)
            .to(to)
            .subject("subject")
            .body("body".to_string())
            .unwrap();
        email.headers().to_string()
    }

    #[test]
    fn from_header_renders_display_name() {
        let m = build_sender("bot@homelab.local", Some("gh-release-notify")).unwrap();
        let rendered = rendered_from_header(m);
        assert!(
            rendered.contains("From: gh-release-notify <bot@homelab.local>"),
            "rendered headers were: {rendered}"
        );
    }

    #[test]
    fn non_ascii_display_name_is_rfc2047_encoded() {
        let m = build_sender("bot@homelab.local", Some("Überwacher")).unwrap();
        let rendered = rendered_from_header(m);
        assert!(
            rendered.contains("=?utf-8?"),
            "expected RFC 2047 encoded display name, got: {rendered}"
        );
    }

    #[test]
    fn from_header_unchanged_without_name() {
        let m = build_sender("bot@homelab.local", None).unwrap();
        let rendered = rendered_from_header(m);
        assert!(
            rendered.contains("From: bot@homelab.local\r\n"),
            "unset-name From header changed: {rendered}"
        );
    }

    #[test]
    fn from_header_escapes_embedded_quote() {
        let m = build_sender("bot@homelab.local", Some("Foo \" Bar")).unwrap();
        let rendered = rendered_from_header(m);
        assert!(
            rendered.contains("\\\""),
            "expected the embedded quote to be escaped, got: {rendered}"
        );
    }

    #[test]
    fn from_header_renders_boundary_length_name() {
        let name = "a".repeat(128);
        let m = build_sender("bot@homelab.local", Some(&name)).unwrap();
        let rendered = rendered_from_header(m);
        assert!(
            rendered.contains(&name),
            "128-char display name was not rendered intact"
        );
        assert!(rendered.contains("<bot@homelab.local>"));
    }
}
