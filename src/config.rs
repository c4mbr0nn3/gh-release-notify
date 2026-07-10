use anyhow::{anyhow, bail, Result};
use cron::Schedule;
use serde::Deserialize;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub poll_interval_seconds: u64,
    pub state_path: String,
    pub sender: String,
    pub repos: Vec<String>,
    pub recipients: Vec<String>,
    pub smtp: SmtpConfig,
    #[serde(default)]
    #[allow(dead_code)]
    pub cron_expression: Option<String>,
    #[serde(skip)]
    #[allow(dead_code)]
    pub cron_schedule: Option<Schedule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub encryption: Encryption,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Encryption {
    StartTls,
    Tls,
    None,
}

impl Config {
    pub fn load(path: &str) -> Result<Config> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("failed to read config file {path}: {e}"))?;
        let mut cfg: Config =
            toml::from_str(&raw).map_err(|e| anyhow!("failed to parse config file {path}: {e}"))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn smtp_password(&self) -> String {
        env::var("SMTP_PASSWORD").unwrap_or_else(|_| self.smtp.password.clone())
    }

    pub fn github_token(&self) -> Option<String> {
        match env::var("GITHUB_TOKEN") {
            Ok(v) if !v.is_empty() => Some(v),
            _ => None,
        }
    }

    fn validate(&mut self) -> Result<()> {
        if self.poll_interval_seconds < 60 {
            bail!("poll_interval_seconds must be >= 60");
        }
        let repo_re = regex::Regex::new(r"^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$").unwrap();
        if self.repos.is_empty() {
            bail!("repos must not be empty");
        }
        for r in &self.repos {
            if !repo_re.is_match(r) {
                bail!("invalid repo '{r}', expected 'owner/repo'");
            }
        }
        if self.recipients.is_empty() {
            bail!("recipients must not be empty");
        }
        let email_re = regex::Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").unwrap();
        if !email_re.is_match(&self.sender) {
            bail!("sender '{}' is not a valid email", self.sender);
        }
        for r in &self.recipients {
            if !email_re.is_match(r) {
                bail!("recipient '{r}' is not a valid email");
            }
        }
        if self.smtp.host.is_empty() {
            bail!("smtp.host must not be empty");
        }
        if self.smtp.port == 0 {
            bail!("smtp.port must be > 0");
        }
        if !self.smtp.username.is_empty() && self.smtp_password().is_empty() {
            bail!("smtp.username is set but no password provided (set [smtp].password or SMTP_PASSWORD env)");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_config(dir: &std::path::Path, contents: &str) -> std::path::PathBuf {
        let p = dir.join("config.toml");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        p
    }

    const VALID: &str = r#"
poll_interval_seconds = 3600
state_path = "./state.json"
sender = "bot@homelab.local"
repos = ["fosrl/pangolin", "fosrl/newt"]
recipients = ["you@example.com"]

[smtp]
host = "smtp.example.com"
port = 587
encryption = "starttls"
username = "postmaster"
password = "secret"
"#;

    #[test]
    fn parses_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_config(dir.path(), VALID);
        let cfg = Config::load(p.to_str().unwrap()).unwrap();
        assert_eq!(cfg.poll_interval_seconds, 3600);
        assert_eq!(cfg.state_path, "./state.json");
        assert_eq!(cfg.sender, "bot@homelab.local");
        assert_eq!(cfg.repos, vec!["fosrl/pangolin", "fosrl/newt"]);
        assert_eq!(cfg.recipients, vec!["you@example.com"]);
        assert_eq!(cfg.smtp.host, "smtp.example.com");
        assert_eq!(cfg.smtp.port, 587);
        assert_eq!(cfg.smtp.encryption, Encryption::StartTls);
        assert_eq!(cfg.smtp.username, "postmaster");
        assert_eq!(cfg.smtp.password, "secret");
    }

    #[test]
    fn parses_config_without_cron() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_config(dir.path(), VALID);
        let cfg = Config::load(p.to_str().unwrap()).unwrap();
        assert!(cfg.cron_expression.is_none());
        assert!(cfg.cron_schedule.is_none());
    }

    #[test]
    fn rejects_poll_interval_below_60() {
        let dir = tempfile::tempdir().unwrap();
        let bad = VALID.replace("poll_interval_seconds = 3600", "poll_interval_seconds = 30");
        let p = write_config(dir.path(), &bad);
        let err = Config::load(p.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("poll_interval_seconds"));
    }

    #[test]
    fn rejects_empty_repos() {
        let dir = tempfile::tempdir().unwrap();
        let bad = VALID.replace("repos = [\"fosrl/pangolin\", \"fosrl/newt\"]", "repos = []");
        let p = write_config(dir.path(), &bad);
        let err = Config::load(p.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("repos"));
    }

    #[test]
    fn rejects_bad_repo_format() {
        let dir = tempfile::tempdir().unwrap();
        let bad = VALID.replace("\"fosrl/newt\"", "\"not-a-slash\"");
        let p = write_config(dir.path(), &bad);
        let err = Config::load(p.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("invalid repo"));
    }

    #[test]
    fn rejects_bad_sender_email() {
        let dir = tempfile::tempdir().unwrap();
        let bad = VALID.replace("bot@homelab.local", "not-an-email");
        let p = write_config(dir.path(), &bad);
        let err = Config::load(p.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("sender"));
    }

    #[test]
    fn rejects_bad_recipient_email() {
        let dir = tempfile::tempdir().unwrap();
        let bad = VALID.replace("you@example.com", "bad");
        let p = write_config(dir.path(), &bad);
        let err = Config::load(p.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("recipient"));
    }

    #[test]
    fn rejects_username_without_password() {
        let dir = tempfile::tempdir().unwrap();
        let bad = VALID.replace("password = \"secret\"", "password = \"\"");
        let p = write_config(dir.path(), &bad);
        env::remove_var("SMTP_PASSWORD");
        let err = Config::load(p.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("password"));
    }

    #[test]
    fn smtp_password_env_overrides_config() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_config(dir.path(), VALID);
        let cfg = Config::load(p.to_str().unwrap()).unwrap();
        env::set_var("SMTP_PASSWORD", "env-secret");
        assert_eq!(cfg.smtp_password(), "env-secret");
        env::remove_var("SMTP_PASSWORD");
    }

    #[test]
    fn github_token_returns_none_when_unset() {
        env::remove_var("GITHUB_TOKEN");
        let dir = tempfile::tempdir().unwrap();
        let p = write_config(dir.path(), VALID);
        let cfg = Config::load(p.to_str().unwrap()).unwrap();
        assert_eq!(cfg.github_token(), None);
    }

    #[test]
    fn github_token_returns_some_when_set() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_config(dir.path(), VALID);
        let cfg = Config::load(p.to_str().unwrap()).unwrap();
        env::set_var("GITHUB_TOKEN", "ghp_xxx");
        assert_eq!(cfg.github_token(), Some("ghp_xxx".to_string()));
        env::remove_var("GITHUB_TOKEN");
    }

    #[test]
    fn parses_encryption_variants() {
        let dir = tempfile::tempdir().unwrap();
        for (val, expected) in [
            ("starttls", Encryption::StartTls),
            ("tls", Encryption::Tls),
            ("none", Encryption::None),
        ] {
            let contents = VALID.replace("starttls", val);
            let p = write_config(dir.path(), &contents);
            let cfg = Config::load(p.to_str().unwrap()).unwrap();
            assert_eq!(cfg.smtp.encryption, expected);
        }
    }

    #[test]
    fn rejects_bad_encryption_value() {
        let dir = tempfile::tempdir().unwrap();
        let bad = VALID.replace("encryption = \"starttls\"", "encryption = \"foo\"");
        let p = write_config(dir.path(), &bad);
        let err = Config::load(p.to_str().unwrap()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("encryption") || msg.contains("unknown variant") || msg.contains("foo"),
            "expected error about encryption, got: {msg}"
        );
    }

    #[test]
    fn rejects_port_zero() {
        let dir = tempfile::tempdir().unwrap();
        let bad = VALID.replace("port = 587", "port = 0");
        let p = write_config(dir.path(), &bad);
        let err = Config::load(p.to_str().unwrap()).unwrap_err();
        assert!(
            err.to_string().contains("port"),
            "expected error about port, got: {}",
            err
        );
    }
}
