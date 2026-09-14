#[allow(dead_code)]
use anyhow::Result;
use serde::Deserialize;
use std::fmt;
use tracing::info;

use crate::config::{Config, Encryption};

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
pub fn apply(path: &str, edit: &ConfigEdit) -> Result<Config, SaveError> {
    for (name, present) in [
        ("state_path", edit.state_path.is_some()),
        ("ui.bind_addr", edit.ui_bind_addr.is_some()),
        ("ui.port", edit.ui_port.is_some()),
        ("ui.trust_proxy_auth", edit.trust_proxy_auth.is_some()),
    ] {
        if present {
            return Err(SaveError::Invalid(format!(
                "{name} is read-only and cannot be set through the UI"
            )));
        }
    }

    let raw = std::fs::read_to_string(path)
        .map_err(|e| classify(&e, &format!("failed to read {path}")))?;
    let mut doc: toml_edit::DocumentMut = raw
        .parse()
        .map_err(|e| SaveError::Invalid(format!("failed to parse {path}: {e}")))?;

    doc["poll_interval_seconds"] = toml_edit::value(edit.poll_interval_seconds as i64);
    doc["sender"] = toml_edit::value(edit.sender.as_str());
    doc["recipients"] = string_array(&edit.recipients);
    doc["repos"] = string_array(&edit.repos);
    match &edit.cron_expression {
        Some(expr) => doc["cron_expression"] = toml_edit::value(expr.as_str()),
        None => {
            doc.remove("cron_expression");
        }
    }

    if doc.get("smtp").is_none() {
        doc["smtp"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    doc["smtp"]["host"] = toml_edit::value(edit.smtp_host.as_str());
    doc["smtp"]["port"] = toml_edit::value(edit.smtp_port as i64);
    doc["smtp"]["encryption"] = toml_edit::value(encryption_str(edit.smtp_encryption));
    doc["smtp"]["username"] = toml_edit::value(edit.smtp_username.as_str());

    if let Some(pw) = &edit.smtp_password {
        if !pw.is_empty() {
            doc["smtp"]["password"] = toml_edit::value(pw.as_str());
        }
    }

    if let Some(token) = &edit.admin_token {
        if doc.get("ui").is_none() {
            doc["ui"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        doc["ui"]["admin_token"] = toml_edit::value(token.as_str());
    }

    let rendered = doc.to_string();
    let mut cfg: Config = toml::from_str(&rendered)
        .map_err(|e| SaveError::Invalid(format!("edited config does not parse: {e}")))?;
    cfg.config_path = path.to_string();
    cfg.validate()
        .map_err(|e| SaveError::Invalid(e.to_string()))?;

    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, &rendered).map_err(|e| classify(&e, &format!("failed to write {tmp}")))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| classify(&e, &format!("failed to rename {tmp} -> {path}")))?;

    info!("config updated at {path}");
    Ok(cfg)
}

#[allow(dead_code)]
fn classify(e: &std::io::Error, ctx: &str) -> SaveError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => {
            SaveError::Unwritable(format!("{ctx}: {e}"))
        }
        _ => SaveError::Io(format!("{ctx}: {e}")),
    }
}

#[allow(dead_code)]
fn string_array(items: &[String]) -> toml_edit::Item {
    let mut arr = toml_edit::Array::new();
    for i in items {
        arr.push(i.as_str());
    }
    toml_edit::Item::Value(toml_edit::Value::Array(arr))
}

#[allow(dead_code)]
fn encryption_str(e: Encryption) -> &'static str {
    match e {
        Encryption::StartTls => "starttls",
        Encryption::Tls => "tls",
        Encryption::None => "none",
    }
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
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("password = \"secret\""));
    }

    #[test]
    fn absent_secret_fields_leave_values_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let edit = edit_from(&path);
        apply(&path, &edit).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("password = \"secret\""));
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
        assert!(!std::fs::read_to_string(&path)
            .unwrap()
            .contains("evil.json"));
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_path_returns_unwritable_not_panic() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let edit = edit_from(&path);
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(dir.path(), perms).unwrap();
        let result = apply(&path, &edit);
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dir.path(), perms).unwrap();
        assert!(
            matches!(result, Err(SaveError::Unwritable(_))),
            "got {result:?}"
        );
    }
}
