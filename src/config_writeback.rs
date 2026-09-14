use serde::Deserialize;
use std::fmt;

use crate::config::Encryption;

#[derive(Debug)]
pub enum SaveError {
    Unwritable(String),
    Invalid(String),
    Io(String),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::Unwritable(m) => write!(f, "config file is not writable: {m}"),
            SaveError::Invalid(m) => write!(f, "invalid config: {m}"),
            SaveError::Io(m) => write!(f, "config I/O error: {m}"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigEdit {
    pub poll_interval_seconds: u64,
    #[serde(default)]
    pub cron_expression: Option<String>,
    pub sender: String,
    pub repos: Vec<String>,
    pub recipients: Vec<String>,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_encryption: Encryption,
    pub smtp_username: String,
    #[serde(default)]
    pub smtp_password: Option<String>,
    #[serde(default)]
    pub admin_token: Option<String>,
    #[serde(default)]
    pub ui_bind_addr: Option<String>,
    #[serde(default)]
    pub ui_port: Option<u16>,
    #[serde(default)]
    pub trust_proxy_auth: Option<bool>,
    #[serde(default)]
    pub state_path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> &'static str {
        r#"
# schedule comment that must survive
poll_interval_seconds = 3600
state_path = "./state.json"
sender = "bot@homelab.local"
repos = ["fosrl/pangolin", "fosrl/newt"]
recipients = ["you@example.com"]

# smtp comment that must survive
[smtp]
host = "smtp.example.com"
port = 587
encryption = "starttls"
username = "postmaster"
password = "secret"
"#
    }

    fn write(dir: &std::path::Path, contents: &str) -> String {
        let p = dir.join("config.toml");
        std::fs::write(&p, contents).unwrap();
        p.to_str().unwrap().to_string()
    }

    fn edit_from(cfg_path: &str) -> ConfigEdit {
        let cfg = crate::config::Config::load(cfg_path).unwrap();
        ConfigEdit {
            poll_interval_seconds: cfg.poll_interval_seconds,
            cron_expression: cfg.cron_expression.clone(),
            sender: cfg.sender.clone(),
            repos: cfg.repos.clone(),
            recipients: cfg.recipients.clone(),
            smtp_host: cfg.smtp.host.clone(),
            smtp_port: cfg.smtp.port,
            smtp_encryption: cfg.smtp.encryption,
            smtp_username: cfg.smtp.username.clone(),
            smtp_password: None,
            admin_token: None,
            ui_bind_addr: None,
            ui_port: None,
            trust_proxy_auth: None,
            state_path: None,
        }
    }

    #[test]
    fn preserves_comments_and_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let mut edit = edit_from(&path);
        edit.poll_interval_seconds = 7200;
        apply(&path, &edit).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# schedule comment that must survive"));
        assert!(after.contains("# smtp comment that must survive"));
        assert!(after.contains("poll_interval_seconds = 7200"));
        let sched = after.find("# schedule comment").unwrap();
        let smtp = after.find("# smtp comment").unwrap();
        assert!(sched < smtp, "original key order was not preserved");
    }

    #[test]
    fn invalid_new_config_is_rejected_before_write() {
        let dir = tempfile::tempdir().unwrap();
        let original = sample_config();
        let path = write(dir.path(), original);
        let mut edit = edit_from(&path);
        edit.poll_interval_seconds = 30;
        let err = apply(&path, &edit).unwrap_err();
        assert!(matches!(err, SaveError::Invalid(_)), "got {err:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn save_is_atomic_and_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let mut edit = edit_from(&path);
        edit.sender = "other@homelab.local".to_string();
        apply(&path, &edit).unwrap();
        assert!(!dir.path().join("config.toml.tmp").exists());
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("other@homelab.local"));
    }

    #[test]
    fn empty_smtp_password_leaves_secret_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let mut edit = edit_from(&path);
        edit.smtp_password = Some(String::new());
        apply(&path, &edit).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("password = \"secret\""));
    }

    #[test]
    fn absent_secret_fields_leave_values_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let edit = edit_from(&path);
        apply(&path, &edit).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("password = \"secret\""));
    }

    #[test]
    fn round_trips_through_config_validate() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let mut edit = edit_from(&path);
        edit.cron_expression = Some("0 */6 * * *".to_string());
        let cfg = apply(&path, &edit).unwrap();
        assert_eq!(cfg.poll_interval_seconds, 3600);
        assert!(cfg.cron_schedule.is_some());
    }

    #[test]
    fn rejects_read_only_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let mut edit = edit_from(&path);
        edit.state_path = Some("/tmp/evil.json".to_string());
        let err = apply(&path, &edit).unwrap_err();
        assert!(matches!(err, SaveError::Invalid(_)), "got {err:?}");
        assert!(!std::fs::read_to_string(&path).unwrap().contains("evil.json"));
    }
}