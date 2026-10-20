use anyhow::Result;
use serde::Deserialize;
use std::fmt;
use tracing::info;

use crate::config::{Config, Encryption};

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
    #[serde(default)]
    pub sender_name: Option<String>,
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
    match edit
        .sender_name
        .as_ref()
        .map(|n| n.trim())
        .filter(|n| !n.is_empty())
    {
        Some(trimmed) => doc["sender_name"] = toml_edit::value(trimmed),
        None => {
            doc.remove("sender_name");
        }
    }
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
    crate::config::apply_ui_env_overrides(&mut cfg)
        .map_err(|e| SaveError::Invalid(e.to_string()))?;
    cfg.validate()
        .map_err(|e| SaveError::Invalid(e.to_string()))?;

    let tmp = unique_tmp_path(path);
    if let Err(e) = write_private(&tmp, &rendered) {
        let _ = std::fs::remove_file(&tmp);
        return Err(classify(&e, &format!("failed to write {tmp}")));
    }
    copy_mode(path, &tmp);
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(classify(&e, &format!("failed to rename {tmp} -> {path}")));
    }

    info!("config updated at {path}");
    Ok(cfg)
}

fn write_private(path: &str, contents: &str) -> std::io::Result<()> {
    use std::io::Write;

    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(contents.as_bytes())?;
    f.sync_all()
}

fn unique_tmp_path(path: &str) -> String {
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pid = std::process::id();
    format!("{path}.tmp.{pid}.{seq}")
}

fn copy_mode(from: &str, to: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(from) {
            let mode = meta.permissions().mode() & 0o777;
            let _ = std::fs::set_permissions(to, std::fs::Permissions::from_mode(mode));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (from, to);
    }
}

fn classify(e: &std::io::Error, ctx: &str) -> SaveError {
    #[cfg(unix)]
    if e.raw_os_error() == Some(16) {
        return SaveError::Unwritable(format!(
            "{ctx}: the config path appears to be a bind-mounted file; mount its parent directory instead"
        ));
    }
    match e.kind() {
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem => {
            SaveError::Unwritable(format!("{ctx}: {e}"))
        }
        _ => SaveError::Io(format!("{ctx}: {e}")),
    }
}

fn string_array(items: &[String]) -> toml_edit::Item {
    let mut arr = toml_edit::Array::new();
    for i in items {
        arr.push(i.as_str());
    }
    toml_edit::Item::Value(toml_edit::Value::Array(arr))
}

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
    use crate::ENV_LOCK;

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
            sender_name: cfg.sender_name.clone(),
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
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    #[test]
    fn sender_name_round_trips_intact() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let contents = std::fs::read_to_string(&path).unwrap();
        let with_name = contents.replace(
            "sender = \"bot@homelab.local\"",
            "sender = \"bot@homelab.local\"\nsender_name = \"Homelab Bot\"",
        );
        std::fs::write(&path, &with_name).unwrap();
        let edit = edit_from(&path);
        apply(&path, &edit).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("sender_name = \"Homelab Bot\""), "{after}");
    }

    #[test]
    fn empty_or_whitespace_sender_name_removes_key() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let contents = std::fs::read_to_string(&path).unwrap();
        let with_name = contents.replace(
            "sender = \"bot@homelab.local\"",
            "sender = \"bot@homelab.local\"\nsender_name = \"Homelab Bot\"",
        );
        std::fs::write(&path, &with_name).unwrap();

        let mut edit = edit_from(&path);
        edit.sender_name = None;
        apply(&path, &edit).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(!after.contains("sender_name"), "{after}");

        let mut edit = edit_from(&path);
        edit.sender_name = Some("   ".to_string());
        apply(&path, &edit).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(!after.contains("sender_name"), "{after}");
    }

    #[test]
    fn new_sender_name_is_written_and_validates() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let mut edit = edit_from(&path);
        edit.sender_name = Some("  GH Releases  ".to_string());
        let cfg = apply(&path, &edit).unwrap();
        assert_eq!(cfg.sender_name.as_deref(), Some("GH Releases"));
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("sender_name = \"GH Releases\""));
    }

    #[test]
    fn invalid_sender_name_fails_and_leaves_file_untouched() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let original = sample_config();
        let path = write(dir.path(), original);
        let mut edit = edit_from(&path);
        edit.sender_name = Some("x".repeat(129));
        let err = apply(&path, &edit).unwrap_err();
        assert!(matches!(err, SaveError::Invalid(_)), "got {err:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(!dir.path().join("config.toml.tmp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn save_preserves_file_mode() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let edit = edit_from(&path);
        apply(&path, &edit).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let _guard = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), sample_config());
        let edit = edit_from(&path);
        apply(&path, &edit).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn rename_onto_mount_point_reports_mount_cause() {
        let err = classify(
            &std::io::Error::from_raw_os_error(16),
            "failed to rename /config/config.toml.tmp.1.1 -> /config/config.toml",
        );
        match err {
            SaveError::Unwritable(m) => {
                assert!(m.contains("bind-mounted file"), "{m}");
                assert!(m.contains("directory"), "{m}");
            }
            other => panic!("expected Unwritable, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_path_returns_unwritable_not_panic() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
