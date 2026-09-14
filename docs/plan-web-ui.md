# Web UI Implementation Plan

**Date:** 2026-09-14
**Specs:** `docs/SPEC-config-writeback.md`, `docs/SPEC-web-ui.md`
**Capability map:** `docs/capability-map.md`
**Task list:** `tasks/todo.md`

**Goal:** Serve a lightweight embedded web UI from the daemon that shows
read-only status and edits `config.toml` (interval, cron, repos, recipients,
sender, SMTP settings, admin token) with changes applied without a restart,
gated by exactly one of: an admin token, a trusted proxy header, or nothing.

**Architecture:** Two modules in dependency order. `config-writeback` adds
comment-preserving atomic config mutation (`toml_edit` → re-parse → validate →
tmp+rename) with no HTTP dependency. `web-ui` adds an axum router, an auth
middleware with three exclusive modes, and a single-file vanilla-JS page
embedded with `include_str!`. Runtime reload is a `tokio::sync::watch` channel
carrying `Arc<Config>`; status flows the other way on a second watch channel
carrying `Arc<StatusSnapshot>`, so `StateStore` keeps its current ownership and
`src/state.rs` is not modified.

**Tech Stack:** Rust 2021, tokio (full), axum 0.8, subtle 2, getrandom 0.4,
toml_edit 0.25, serde/serde_json, toml, anyhow, tracing, chrono, cron,
tempfile (dev), tower (dev, already in lock).

## Global Constraints

- No comments in Rust source files unless explicitly requested.
  `config.example.toml`, `.env.example`, `README.md`, `docker-compose.yml`,
  `Dockerfile`, and `AGENTS.md` are documentation files and MAY contain
  comments.
- No `unwrap`/`expect`/`panic` in non-test code. The existing
  `Regex::new(...).unwrap()` calls in `config.rs` and
  `expect("install SIGTERM handler")` in `main.rs` are the only sanctioned
  exceptions — do not add new ones.
- All fallible operations return `Result`. Handle with `?` at task boundaries
  or log-and-continue; never swallow errors silently.
- Conventional Commits: `<type>: <subject>`, subject <= 72 chars, imperative
  mood, no trailing period, **no body**.
- Verification gate before every commit: `cargo fmt && cargo clippy -- -D warnings && cargo test`.
- **Never** add `rand`. Session entropy comes from `getrandom::fill`.
- **Never** return a secret value from any endpoint or write one to a log.
- The UI's only filesystem write target is the config path. Never `state_path`.
- `src/state.rs` must not be modified by this work.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `Cargo.toml` | Modify | Add `toml_edit`, `axum`, `subtle`, `getrandom` |
| `src/config.rs` | Modify | `UiConfig`, `config_path`, `Encryption: Serialize`, validation, tests |
| `src/config_writeback.rs` | Create | `ConfigEdit`, `SaveError`, `apply()`, tests |
| `src/scheduler.rs` | Modify | `watch<Arc<Config>>` receive, `changed()` branch, status publish |
| `src/main.rs` | Modify | Wire channels, spawn `ui::serve`, startup auth log |
| `src/ui/mod.rs` | Create | Router, `AppState`, handlers, tests |
| `src/ui/auth.rs` | Create | Middleware, sessions, login limiter |
| `src/ui/assets.rs` | Create | `include_str!` constants |
| `src/ui/index.html` | Create | The page |
| `src/ui/app.js` | Create | Client logic |
| `src/ui/pico.classless.min.css` | Create (vendored) | Pico 2.1.1 classless, MIT header intact |
| `config.example.toml` | Modify | Document `[ui]` |
| `.env.example` | Modify | Document `ADMIN_TOKEN` |
| `docker-compose.yml` | Modify | rw config mount, loopback port mapping, `ADMIN_TOKEN` |
| `README.md` | Modify | UI docs, auth modes, security posture, third-party notice |
| `AGENTS.md` | Modify | Scope list + new `## Web UI` section |

---

## Phase A — `config-writeback`

### Task A1: Add the write-back dependencies

**Files:** Modify `Cargo.toml:6-19`

**Interfaces:** Produces: `toml_edit` available. (`getrandom`, `subtle`, and
`axum` are added in their own tasks.)

- [ ] **Step 1: Add `toml_edit`**

After the `toml = "1"` line (`Cargo.toml:11`), add:

```toml
toml_edit = "0.25"
```

- [ ] **Step 2: Verify**

Run: `cargo build`
Expected: BUILD SUCCESS. `Cargo.lock` gains no `toml_edit`-only package family
beyond what `toml` already pulls.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add toml_edit for comment-preserving config writes"
```

---

### Task A2: Add `config_path` to `Config` and `Serialize` to `Encryption`

**Files:** Modify `src/config.rs:3`, `src/config.rs:7-19`, `src/config.rs:30-36`,
`src/config.rs:39-46`. Test: `src/config.rs` under `#[cfg(test)]`.

**Interfaces:**
- Produces: `Config.config_path: String` (set by `load()`), and
  `Encryption: Serialize` so the write path can render it as a lowercase string.
- Consumed by: A3's `apply()`, and `web-ui`'s `AppState`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/config.rs` (after `parses_valid_config`,
around line 157):

```rust
#[test]
fn load_records_config_path() {
    let dir = tempfile::tempdir().unwrap();
    let p = write_config(dir.path(), VALID);
    let cfg = Config::load(p.to_str().unwrap()).unwrap();
    assert_eq!(cfg.config_path, p.to_str().unwrap());
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test load_records_config_path`
Expected: COMPILE ERROR — `no field config_path on type Config`.

- [ ] **Step 3: Implement**

In `src/config.rs`, change the serde import (line 3) to:

```rust
use serde::{Deserialize, Serialize};
```

Add the field to `Config` (after `cron_schedule`, `src/config.rs:18`):

```rust
    #[serde(skip)]
    pub config_path: String,
```

Add `Serialize` to the `Encryption` derive (`src/config.rs:30`):

```rust
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
```

Set the field in `load()` (replace the body, `src/config.rs:39-46`):

```rust
    pub fn load(path: &str) -> Result<Config> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("failed to read config file {path}: {e}"))?;
        let mut cfg: Config =
            toml::from_str(&raw).map_err(|e| anyhow!("failed to parse config file {path}: {e}"))?;
        cfg.config_path = path.to_string();
        cfg.validate()?;
        Ok(cfg)
    }
```

`config_path` must be set **before** `validate()` so a validator that needs the
path can rely on it, and `#[serde(skip)]` guarantees it is never deserialized
from TOML.

- [ ] **Step 4: Verify**

Run: `cargo test config`
Expected: ALL PASS, including the new test.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: carry config path on Config and serialize Encryption"
```

---

### Task A3: Create `src/config_writeback.rs` with `SaveError` and `ConfigEdit`

**Files:** Create `src/config_writeback.rs`. Modify `src/main.rs:1-5`.

**Interfaces:**
- Consumes: `Config.config_path` (A2), `Encryption: Serialize` (A2).
- Produces: `pub enum SaveError { Unwritable(String), Invalid(String), Io(String) }`
  with `Display`; `pub struct ConfigEdit` with public fields; `pub fn apply(path: &str, edit: &ConfigEdit) -> Result<Config, SaveError>`.

- [ ] **Step 1: Write the failing test**

Create `src/config_writeback.rs` with only the test module and the types needed
to compile it. Add:

```rust
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
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test config_writeback`
Expected: COMPILE ERROR — `SaveError`, `ConfigEdit`, `apply` undefined.

- [ ] **Step 3: Declare the module**

In `src/main.rs`, add to the module list (`src/main.rs:1-5`), keeping
alphabetical order:

```rust
mod config;
mod config_writeback;
mod github;
mod notify;
mod scheduler;
mod state;
```

- [ ] **Step 4: Implement the types**

At the top of `src/config_writeback.rs`, above the test module:

```rust
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
```

`Encryption` needs `Deserialize` on `ConfigEdit` — it already has it
(`src/config.rs:30`).

- [ ] **Step 5: Verify it compiles**

Run: `cargo build`
Expected: BUILD SUCCESS.

- [ ] **Step 6: Commit**

```bash
git add src/config_writeback.rs src/main.rs
git commit -m "feat: add ConfigEdit and SaveError for config write-back"
```

---

### Task A4: Implement `config_writeback::apply()`

**Files:** Modify `src/config_writeback.rs`.

**Interfaces:**
- Consumes: `ConfigEdit` (A3), `Config::validate` (`src/config.rs:59`),
  `Config.config_path` (A2).
- Produces: `pub fn apply(path: &str, edit: &ConfigEdit) -> Result<Config, SaveError>`
  — writes the file atomically and returns the validated new `Config`.

- [ ] **Step 1: Run the failing tests**

Run: `cargo test config_writeback`
Expected: FAIL — `apply` not found.

- [ ] **Step 2: Implement `apply()`**

Add below the `ConfigEdit` definition:

```rust
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

    match &edit.admin_token {
        Some(token) => {
            if doc.get("ui").is_none() {
                doc["ui"] = toml_edit::Item::Table(toml_edit::Table::new());
            }
            doc["ui"]["admin_token"] = toml_edit::value(token.as_str());
        }
        None => {}
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

fn classify(e: &std::io::Error, ctx: &str) -> SaveError {
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
```

Notes for the implementer:
- `std::io::ErrorKind::ReadOnlyFilesystem` may be unstable on the pinned
  toolchain. If it does not compile, drop that arm and rely on
  `PermissionDenied` only — but keep the name `Unwritable` and the 409 mapping.
- `std::fs::rename` is atomic on the same filesystem, which the tmp file always
  shares with the target.
- Validation happens on the rendered string **before** any write — if the
  implementer is tempted to reorder, don't: the `invalid_new_config_is_rejected_before_write`
  test enforces this.

- [ ] **Step 3: Run the tests**

Run: `cargo test config_writeback`
Expected: ALL PASS.

- [ ] **Step 4: Full suite + clippy**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: ALL PASS, NO WARNINGS.

- [ ] **Step 5: Commit**

```bash
git add src/config_writeback.rs
git commit -m "feat: implement atomic comment-preserving config write-back"
```

---

### Task A5: Add the unwritable-path test

**Files:** Modify `src/config_writeback.rs` (test module).

**Interfaces:** Consumes: `apply()` (A4). Produces: coverage for the
read-only-mount path that `docker-compose.yml` can trigger.

- [ ] **Step 1: Add the test**

In the test module:

```rust
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
    assert!(matches!(result, Err(SaveError::Unwritable(_))), "got {result:?}");
}
```

- [ ] **Step 2: Run**

Run: `cargo test config_writeback`
Expected: ALL PASS.

- [ ] **Step 3: Commit**

```bash
git add src/config_writeback.rs
git commit -m "test: cover unwritable config path error mapping"
```

---

## Phase B — `web-ui`

### Task B1: Add the UI dependencies

**Files:** Modify `Cargo.toml:6-19`.

**Interfaces:** Produces: `axum`, `subtle`, `getrandom` available. `tower` and
`tower-http` are already in `Cargo.lock` via `reqwest`; add `tower` to
`[dev-dependencies]` for the test harness.

- [ ] **Step 1: Add dependencies**

```toml
axum = "0.8"
subtle = "2"
getrandom = "0.4"
```

And under `[dev-dependencies]`:

```toml
tower = { version = "0.5", features = ["util"] }
```

The `util` feature provides `ServiceExt::oneshot`, which the middleware tests
need.

- [ ] **Step 2: Verify**

Run: `cargo build && cargo test`
Expected: BUILD SUCCESS, existing tests PASS. This is the first build with the
new package group and will take longer than usual.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add axum, subtle, and getrandom for the web UI"
```

---

### Task B2: Add `UiConfig` and `[ui]` validation

**Files:** Modify `src/config.rs:7-19`, add `UiConfig` struct, extend
`validate()` (`src/config.rs:59-109`). Test: `src/config.rs`.

**Interfaces:**
- Produces: `Config.ui: UiConfig` with `bind_addr`, `port`, `admin_token`,
  `trust_proxy_auth`; `Config::admin_token() -> String` resolving `ADMIN_TOKEN`;
  `Config::ui_auth_mode() -> AuthMode`; and `pub enum AuthMode { Token, Proxy, Open }`
  defined in `config.rs`.
- Consumed by: B3 (`main.rs` wiring), B5–B7 (UI handlers).

**Dependency direction (resolved):** `AuthMode` is declared in `src/config.rs`
and re-exported by `src/ui/mod.rs` (`pub use crate::config::AuthMode;`).
`config` must never depend on `ui`, so the enum cannot live in `ui`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn parses_ui_section_with_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let p = write_config(dir.path(), VALID);
    let cfg = Config::load(p.to_str().unwrap()).unwrap();
    assert_eq!(cfg.ui.bind_addr, "127.0.0.1");
    assert_eq!(cfg.ui.port, 8080);
    assert!(cfg.ui.admin_token.is_empty());
    assert!(!cfg.ui.trust_proxy_auth);
}

#[test]
fn rejects_token_and_proxy_auth_together() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    env::remove_var("ADMIN_TOKEN");
    let dir = tempfile::tempdir().unwrap();
    let contents = format!(
        "{VALID}\n[ui]\nadmin_token = \"tok\"\ntrust_proxy_auth = true\n"
    );
    let p = write_config(dir.path(), &contents);
    let err = Config::load(p.to_str().unwrap()).unwrap_err();
    assert!(err.to_string().contains("mutually exclusive"));
}

#[test]
fn rejects_ui_port_zero() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!("{VALID}\n[ui]\nport = 0\n");
    let p = write_config(dir.path(), &contents);
    let err = Config::load(p.to_str().unwrap()).unwrap_err();
    assert!(err.to_string().contains("port"));
}

#[test]
fn rejects_unparseable_bind_addr() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!("{VALID}\n[ui]\nbind_addr = \"not-an-ip\"\n");
    let p = write_config(dir.path(), &contents);
    let err = Config::load(p.to_str().unwrap()).unwrap_err();
    assert!(err.to_string().contains("bind_addr"));
}

#[test]
fn admin_token_env_overrides_config() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let p = write_config(dir.path(), VALID);
    let cfg = Config::load(p.to_str().unwrap()).unwrap();
    env::set_var("ADMIN_TOKEN", "from-env");
    assert_eq!(cfg.admin_token(), "from-env");
    env::remove_var("ADMIN_TOKEN");
}
```

Note: `VALID` has no `[ui]` section, so defaults must come from
`#[serde(default)]`. Because `VALID` ends with `[smtp]` table lines, appending
`[ui]` after it is valid TOML.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test config`
Expected: COMPILE ERROR — `no field ui on type Config`.

- [ ] **Step 3: Implement**

Add the struct and derive (`src/config.rs`, after `Encryption`):

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UiConfig {
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    #[serde(default = "default_ui_port")]
    pub port: u16,
    #[serde(default)]
    pub admin_token: String,
    #[serde(default)]
    pub trust_proxy_auth: bool,
}

fn default_bind_addr() -> String {
    "127.0.0.1".to_string()
}

fn default_ui_port() -> u16 {
    8080
}
```

`UiConfig` must derive `Serialize` because `GET /api/config` renders it.

Add to `Config` (after `config_path`):

```rust
    #[serde(default)]
    pub ui: UiConfig,
```

Add the resolver, next to `smtp_password()` (`src/config.rs:48`):

```rust
    pub fn admin_token(&self) -> String {
        match env::var("ADMIN_TOKEN") {
            Ok(v) if !v.is_empty() => v,
            _ => self.ui.admin_token.clone(),
        }
    }
```

Add validation at the end of `validate()`, before `Ok(())`
(`src/config.rs:108`):

```rust
        if self.ui.port == 0 {
            bail!("ui.port must be > 0");
        }
        if self.ui.bind_addr.parse::<std::net::IpAddr>().is_err() {
            bail!(
                "ui.bind_addr '{}' is not a valid IP address",
                self.ui.bind_addr
            );
        }
        if !self.admin_token().is_empty() && self.ui.trust_proxy_auth {
            bail!(
                "ui.admin_token and ui.trust_proxy_auth are mutually exclusive; \
                 configure exactly one auth mode"
            );
        }
```

Add the auth-mode enum and resolver to `src/config.rs` (near `Encryption`):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Token,
    Proxy,
    Open,
}
```

And on `impl Config`, next to `admin_token()`:

```rust
    pub fn ui_auth_mode(&self) -> AuthMode {
        if !self.admin_token().is_empty() {
            AuthMode::Token
        } else if self.ui.trust_proxy_auth {
            AuthMode::Proxy
        } else {
            AuthMode::Open
        }
    }
```

`Config::validate` rejects the first two being true at once, so this ordering
is unambiguous for any config that passed validation.

- [ ] **Step 4: Run**

Run: `cargo test config && cargo clippy -- -D warnings`
Expected: ALL PASS, NO WARNINGS.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: add [ui] config section with exclusive auth mode validation"
```

---

### Task B3: Convert the scheduler to reload from a watch channel

**Files:** Modify `src/scheduler.rs:13-108`.

**Interfaces:**
- Consumes: `watch::Receiver<Arc<Config>>`, `watch::Sender<Arc<StatusSnapshot>>`.
- Produces: `run(config_rx, github, state, status_tx, shutdown)`;
  `pub struct StatusSnapshot` and `pub struct RepoStatus`.

- [ ] **Step 1: Add the status types and new signature**

In `src/scheduler.rs`, add above `run()`:

```rust
use std::sync::Arc;

use crate::config::Config;

#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoStatus {
    pub repo: String,
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StatusSnapshot {
    pub repos: Vec<RepoStatus>,
    pub last_poll_finished_at: Option<chrono::DateTime<Utc>>,
    pub next_poll_at: Option<chrono::DateTime<Utc>>,
}

pub async fn run(
    config_rx: watch::Receiver<Arc<Config>>,
    github: GithubClient,
    mut state: StateStore,
    status_tx: watch::Sender<Arc<StatusSnapshot>>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        let cfg = config_rx.borrow().clone();
        let interval = Duration::from_secs(cfg.poll_interval_seconds);
        // ... unchanged poll body from src/scheduler.rs:22-66, using `cfg` ...
```

- [ ] **Step 2: Publish status and re-arm the sleep**

Replace the wait section (`src/scheduler.rs:67-105`) with a version that:

1. Computes the next occurrence exactly as today (cron branch or interval).
2. Publishes the snapshot **before** sleeping:

```rust
        let now = Utc::now();
        let next_poll_at = match &cfg.cron_schedule {
            Some(schedule) => schedule.after(&now).next(),
            None => Some(now + chrono::Duration::seconds(interval.as_secs() as i64)),
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
```

3. Adds the config-change branch to each `tokio::select!`:

```rust
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
```

Because `config_rx` is borrowed with `borrow()` rather than `borrow_and_update()`
at the top of the loop, `changed()` still sees the pending change and fires —
verify this in Step 3, since it is the one subtlety in this task.

4. Builds the mailer per send instead of taking one as a parameter. Remove the
`mailer: Mailer` parameter and at the send site replace
`mailer.send_new_release(...)` with:

```rust
                        let mailer = match Mailer::new(&cfg) {
                            Ok(m) => m,
                            Err(e) => {
                                error!("failed to build mailer for {repo}: {e}");
                                continue;
                            }
                        };
                        match mailer.send_new_release(&release, repo, &cfg.recipients).await {
```

- [ ] **Step 3: Verify the changed() semantics**

Run: `cargo build`
Expected: BUILD SUCCESS. If `config_rx.changed()` never fires because the value
was marked seen, switch the loop head to `config_rx.borrow_and_update().clone()`
and re-verify. Document the resolution in the task report.

- [ ] **Step 4: Full suite + clippy**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: ALL PASS, NO WARNINGS. `src/state.rs` must be unchanged:
`git diff --stat src/state.rs` shows nothing.

- [ ] **Step 5: Commit**

```bash
git add src/scheduler.rs
git commit -m "refactor: reload scheduler config from watch channel and publish status"
```

---

### Task B4: Create the auth middleware module

**Files:** Create `src/ui/auth.rs`, `src/ui/assets.rs`, `src/ui/mod.rs` (skeleton).
Modify `src/main.rs`.

**Interfaces:**
- Produces: `pub struct Sessions`, `pub struct LoginLimiter`,
  `pub async fn require_auth(State, Request, Next) -> Response`,
  `pub const PUBLIC_ROUTES: &[&str]`, `pub enum AuthMode`.
- Consumed by: B6/B7 handlers and the test suite.

- [ ] **Step 1: Write the failing tests**

In `src/ui/mod.rs` (test module), starting with the cases that need no
handlers beyond the router:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn token_app(token: &str) -> axum::Router {
        test_app(test_config(token, false)).await
    }

    #[tokio::test]
    async fn unauthenticated_request_is_rejected() {
        let app = token_app("secret").await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_token_is_rejected() { /* Bearer nope -> 401 */ }

    #[tokio::test]
    async fn valid_bearer_token_is_accepted() { /* Bearer secret -> 200 */ }

    #[tokio::test]
    async fn public_routes_need_no_auth() { /* /, /login, /healthz -> not 401 */ }

    #[tokio::test]
    async fn proxy_mode_accepts_remote_user() { /* Remote-User -> 200; absent -> 401 */ }

    #[tokio::test]
    async fn open_mode_allows_everything() { /* "" token -> 200 */ }

    #[tokio::test]
    async fn mutation_without_custom_header_is_rejected() { /* -> 400 */ }

    #[tokio::test]
    async fn no_secret_appears_in_config_payload() { /* THE invariant test */ }
}
```

Provide a `test_app(cfg: Config) -> Router` helper in the test module that
builds the real router with a `tempfile` config file backing it.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test ui`
Expected: COMPILE ERROR — `test_app`, `Router` undefined.

- [ ] **Step 3: Create `src/ui/assets.rs`**

```rust
pub const INDEX_HTML: &str = include_str!("index.html");
pub const APP_JS: &str = include_str!("app.js");
pub const PICO_CSS: &str = include_str!("pico.classless.min.css");
```

Create placeholder `index.html`, `app.js` now so this compiles; B8 replaces
them with real content. The CSS file is fetched in B8 (Task B8 Step 1).

- [ ] **Step 4: Create `src/ui/auth.rs`**

```rust
use std::collections::HashMap;
use std::sync::Mutex;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, Utc};
use subtle::ConstantTimeEq;
use tracing::{debug, warn};

use super::AppState;

pub const PUBLIC_ROUTES: &[&str] = &["/", "/app.js", "/pico.css", "/login", "/healthz"];
pub const CSRF_HEADER: &str = "x-requested-with";
pub const CSRF_VALUE: &str = "gh-release-notify";
pub const SESSION_TTL_HOURS: i64 = 12;
const LOGIN_LIMIT_PER_MINUTE: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Token,
    Proxy,
    Open,
}

#[derive(Default)]
pub struct Sessions {
    map: Mutex<HashMap<String, DateTime<Utc>>>,
}

impl Sessions {
    pub fn create(&self) -> String {
        let mut buf = [0u8; 32];
        if getrandom::fill(&mut buf).is_err() {
            warn!("session entropy unavailable");
            return String::new();
        }
        let id: String = buf.iter().map(|b| format!("{b:02x}")).collect();
        if let Ok(mut m) = self.map.lock() {
            m.retain(|_, exp| *exp > Utc::now());
            m.insert(id.clone(), Utc::now() + Duration::hours(SESSION_TTL_HOURS));
        }
        id
    }

    pub fn validate(&self, id: &str) -> bool {
        let Ok(mut m) = self.map.lock() else {
            return false;
        };
        match m.get(id) {
            Some(exp) if *exp > Utc::now() => true,
            Some(_) => {
                m.remove(id);
                false
            }
            None => false,
        }
    }

    pub fn destroy(&self, id: &str) {
        if let Ok(mut m) = self.map.lock() {
            m.remove(id);
        }
    }
}

#[derive(Default)]
pub struct LoginLimiter {
    attempts: Mutex<HashMap<String, (u32, DateTime<Utc>)>>,
}

impl LoginLimiter {
    pub fn allow(&self, ip: &str) -> bool {
        let Ok(mut m) = self.attempts.lock() else {
            return false;
        };
        let now = Utc::now();
        let entry = m.entry(ip.to_string()).or_insert((0, now));
        if now - entry.1 > Duration::minutes(1) {
            *entry = (1, now);
            return true;
        }
        entry.0 += 1;
        entry.0 <= LOGIN_LIMIT_PER_MINUTE
    }

    pub fn reset(&self, ip: &str) {
        if let Ok(mut m) = self.attempts.lock() {
            m.remove(ip);
        }
    }
}

pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

pub fn session_cookie(req: &Request) -> Option<String> {
    let raw = req.headers().get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|p| p.trim().strip_prefix("grn_session="))
        .next()
        .map(|s| s.to_string())
}

pub async fn require_auth(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    if PUBLIC_ROUTES.contains(&path.as_str()) {
        return next.run(req).await;
    }

    let cfg = state.config_rx.borrow().clone();
    let token = cfg.admin_token();

    let authorized = if cfg.ui.trust_proxy_auth {
        let ok = req
            .headers()
            .get("remote-user")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| !v.is_empty());
        if ok {
            debug!("proxy auth accepted for {path}");
        }
        ok
    } else if !token.is_empty() {
        let via_cookie = session_cookie(&req)
            .is_some_and(|id| state.sessions.validate(&id));
        let via_bearer = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|presented| constant_time_eq(presented, &token));
        via_cookie || via_bearer
    } else {
        true
    };

    if !authorized {
        warn!("unauthorized request to {path}");
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    let is_mutation = req.method() != axum::http::Method::GET;
    if is_mutation {
        let header_ok = req
            .headers()
            .get(CSRF_HEADER)
            .and_then(|v| v.to_str().ok())
            == Some(CSRF_VALUE);
        if !header_ok {
            return (StatusCode::BAD_REQUEST, "missing X-Requested-With header").into_response();
        }
    }

    next.run(req).await
}

pub fn cookie_header(session: &str, secure: bool) -> String {
    let mut c = format!(
        "grn_session={session}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        SESSION_TTL_HOURS * 3600
    );
    if secure {
        c.push_str("; Secure");
    }
    c
}

pub fn clear_cookie_header() -> String {
    "grn_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0".to_string()
}

pub fn is_https(req: &Request) -> bool {
    req.headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        == Some("https")
}
```

Note the `is_mutation` block contains an empty branch for open mode; the
implementer should drop that empty `if` entirely if clippy flags it — the
mutation header requirement applies in all modes, which is the intent.

- [ ] **Step 5: Run**

Run: `cargo test ui && cargo clippy -- -D warnings`
Expected: The auth tests that only need the middleware PASS; handler-dependent
tests are added in B6/B7 and will still error until then. If the module does not
compile without handlers, create minimal stub handlers in B5 first and return to
this step.

- [ ] **Step 6: Commit**

```bash
git add src/ui/auth.rs src/ui/assets.rs src/ui/mod.rs src/main.rs
git commit -m "feat: add UI auth middleware with exclusive token/proxy/open modes"
```

---

### Task B5: Create `AppState` and the router skeleton

**Files:** Modify `src/ui/mod.rs`, `src/main.rs`.

**Interfaces:**
- Consumes: `Sessions`, `LoginLimiter` (B4).
- Produces: `pub struct AppState` (cloneable, holds `config_rx`, `config_tx`,
  `status_rx`, `config_path`, `sessions`, `limiter`, `started_at`);
  `pub fn router(state: AppState) -> Router`;
  `pub async fn serve(state, shutdown: watch::Receiver<bool>) -> Result<()>`.

**Ordering note:** `auth.rs` (B4) references `super::AppState`, so this task's
`AppState` and module skeleton must exist before B4 compiles. Either do B5
first, or create the `AppState`/`router` skeleton as the first step of B4. The
phase ordering in this document is by dependency, not strict sequence.

- [ ] **Step 1: Implement**

```rust
mod assets;
mod auth;

use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use axum::routing::{get, post};
use axum::{middleware, Router};
use tokio::sync::watch;
use tracing::info;

use crate::config::Config;
use crate::scheduler::StatusSnapshot;

pub use auth::{AuthMode, LoginLimiter, Sessions};

#[derive(Clone)]
pub struct AppState {
    pub config_rx: watch::Receiver<Arc<Config>>,
    pub config_tx: watch::Sender<Arc<Config>>,
    pub status_rx: watch::Receiver<Arc<StatusSnapshot>>,
    pub config_path: String,
    pub sessions: Arc<Sessions>,
    pub limiter: Arc<LoginLimiter>,
    pub started_at: Instant,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/app.js", get(serve_js))
        .route("/pico.css", get(serve_css))
        .route("/login", post(login))
        .route("/healthz", get(healthz))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/logout", post(logout))
        .layer(middleware::from_fn_with_state(state.clone(), auth::require_auth))
        .with_state(state)
}

pub async fn serve(state: AppState, shutdown: watch::Receiver<bool>) -> Result<()> {
    let addr = {
        let cfg = state.config_rx.borrow().clone();
        format!("{}:{}", cfg.ui.bind_addr, cfg.ui.port)
    };
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow!("failed to bind UI on {addr}: {e}"))?;
    info!("ui listening on http://{addr}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            let mut shutdown = shutdown;
            let _ = shutdown.changed().await;
        })
        .await
        .map_err(|e| anyhow!("ui server error: {e}"))
}
```

Handlers referenced here are implemented in B6/B7. Create them as `todo!()`-free
stubs returning `StatusCode::NOT_IMPLEMENTED` in this task, then fill them in.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: BUILD SUCCESS.

- [ ] **Step 3: Commit**

```bash
git add src/ui/mod.rs
git commit -m "feat: add UI AppState, router, and server bootstrap"
```

---

### Task B6: Implement the public handlers

**Files:** Modify `src/ui/mod.rs`.

**Interfaces:** Consumes: `assets::*`. Produces: `GET /`, `/app.js`,
`/pico.css`, `/healthz`, `POST /login`.

- [ ] **Step 1: Implement**

```rust
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

async fn serve_index() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        assets::INDEX_HTML,
    )
        .into_response()
}

async fn serve_js() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        assets::APP_JS,
    )
        .into_response()
}

async fn serve_css() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        assets::PICO_CSS,
    )
        .into_response()
}

async fn healthz(State(state): State<AppState>) -> Response {
    let status = state.status_rx.borrow().clone();
    axum::Json(json!({
        "status": "ok",
        "uptime_seconds": state.started_at.elapsed().as_secs(),
        "next_poll_at": status.next_poll_at,
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
struct LoginBody {
    admin_token: String,
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<LoginBody>,
) -> Response {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .unwrap_or("unknown")
        .trim()
        .to_string();

    if !state.limiter.allow(&ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            axum::Json(json!({"error": "too many attempts"})),
        )
            .into_response();
    }

    let cfg = state.config_rx.borrow().clone();
    let expected = cfg.admin_token();
    if expected.is_empty() || !auth::constant_time_eq(&body.admin_token, &expected) {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error": "unauthorized"})),
        )
            .into_response();
    }

    state.limiter.reset(&ip);
    let session = state.sessions.create();
    let secure = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        == Some("https");
    (
        StatusCode::OK,
        [(header::SET_COOKIE, auth::cookie_header(&session, secure))],
        axum::Json(json!({"ok": true})),
    )
        .into_response()
}
```

- [ ] **Step 2: Verify**

Run: `cargo build && cargo test ui`
Expected: BUILD SUCCESS; `public_routes_need_no_auth` PASS.

- [ ] **Step 3: Commit**

```bash
git add src/ui/mod.rs
git commit -m "feat: add UI static, health, and login handlers"
```

---

### Task B7: Implement `GET`/`PUT /api/config` and logout

**Files:** Modify `src/ui/mod.rs`.

**Interfaces:** Consumes: `config_writeback::apply`, `ConfigEdit`. Produces:
the config and logout endpoints. This task makes the invariant test
(`no_secret_appears_in_config_payload`) pass.

- [ ] **Step 1: Implement `GET /api/config`**

Build the payload per `docs/SPEC-web-ui.md`. **Never** insert
`smtp.password`, `admin_token` value, `SMTP_PASSWORD`, or `GITHUB_TOKEN` into
it. Use `admin_token_set: !cfg.admin_token().is_empty()` and
`env_managed: { smtp_password: env::var("SMTP_PASSWORD").is_ok(), github_token: cfg.github_token().is_some() }`.
Probe writability without writing: attempt to open `format!("{path}.tmp")` for
creation, then remove it, mapping a permission error to `config_writable: false`.

- [ ] **Step 2: Implement `PUT /api/config`**

```rust
async fn put_config(
    State(state): State<AppState>,
    axum::Json(edit): axum::Json<crate::config_writeback::ConfigEdit>,
) -> Response {
    let path = state.config_path.clone();
    let result = tokio::task::spawn_blocking(move || {
        crate::config_writeback::apply(&path, &edit)
    })
    .await;

    match result {
        Ok(Ok(cfg)) => {
            let _ = state.config_tx.send(Arc::new(cfg));
            (StatusCode::OK, axum::Json(json!({"ok": true}))).into_response()
        }
        Ok(Err(crate::config_writeback::SaveError::Invalid(m))) => {
            (StatusCode::BAD_REQUEST, axum::Json(json!({"error": m}))).into_response()
        }
        Ok(Err(crate::config_writeback::SaveError::Unwritable(m))) => {
            (StatusCode::CONFLICT, axum::Json(json!({"error": m}))).into_response()
        }
        Ok(Err(crate::config_writeback::SaveError::Io(m))) => {
            (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(json!({"error": m})))
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({"error": format!("task join error: {e}")})),
        )
            .into_response(),
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(id) = auth::session_cookie_from_headers(&headers) {
        state.sessions.destroy(&id);
    }
    (
        StatusCode::OK,
        [(header::SET_COOKIE, auth::clear_cookie_header())],
        axum::Json(json!({"ok": true})),
    )
        .into_response()
}
```

`AppState` needs a `config_tx: watch::Sender<Arc<Config>>` field — add it in
this task. `send` cannot fail here because the scheduler always holds a
receiver; if the implementer prefers, use `send_replace` to make that
non-failing by construction. `Session::cookie` lookup needs a header-based
helper; refactor `auth::session_cookie` to take `&HeaderMap` and update B4's
call site.

Note: `apply` is blocking filesystem work, so it runs in `spawn_blocking` to
avoid stalling the async runtime — this matters because the daemon shares one
runtime with the scheduler.

- [ ] **Step 3: Verify**

Run: `cargo test ui && cargo clippy -- -D warnings`
Expected: ALL PASS, including `no_secret_appears_in_config_payload`.

- [ ] **Step 4: Commit**

```bash
git add src/ui/mod.rs
git commit -m "feat: add config read/write and logout endpoints"
```

---

### Task B8: Build the frontend assets

**Files:** Create/replace `src/ui/index.html`, `src/ui/app.js`,
`src/ui/pico.classless.min.css`.

**Interfaces:** Consumes: the endpoint shapes from B6/B7. Produces: the page.

- [ ] **Step 1: Vendor Pico CSS**

Download `pico.classless.min.css` version **2.1.1** from the upstream release
and place it at `src/ui/pico.classless.min.css`. **Verify the MIT copyright
header comment is present** (`Pico CSS v2.1.1 — ... Licensed under MIT`); if
the download lacks it, prepend it. Do not hand-modify the CSS.

- [ ] **Step 2: Write `index.html`**

Semantic HTML only (classless CSS does the work). Sections:
1. Login form (token input, `autocomplete="current-password"`), hidden by JS when a session is active or the mode is not `token`.
2. Status `<table>` (repo, last-seen) plus last/next poll lines.
3. Settings `<form>`: interval (number, min 60), cron (text), sender, recipients (rows + add/remove), repos (rows + add/remove), SMTP host/port/encryption `<select>`/username, password (type=password, placeholder "unchanged", never populated).
4. Read-only disabled fields: `state_path`, `bind_addr`, `port`, each with a "restart required" note.
5. Errors rendered inline in an `<output>` element.

- [ ] **Step 3: Write `app.js`**

Vanilla JS: `fetch('/api/config')` on load; render; on submit
`fetch('/api/config', {method:'PUT', headers:{'Content-Type':'application/json','X-Requested-With':'gh-release-notify'}, body})`;
show `{"error":...}` inline; add/remove rows; 401 → show login form; logout
button posts `/api/logout` with the custom header.

The `X-Requested-With` header is **required** on every mutation — omitting it
produces a 400.

- [ ] **Step 4: Verify**

Run: `cargo build`
Expected: BUILD SUCCESS (assets compile into the binary via `include_str!`).
Manually inspect that `index.html` references `/app.js` and `/pico.css` at those
exact paths.

- [ ] **Step 5: Commit**

```bash
git add src/ui/index.html src/ui/app.js src/ui/pico.classless.min.css
git commit -m "feat: add embedded UI page with vendored Pico CSS"
```

---

### Task B9: Wire everything in `main.rs`

**Files:** Modify `src/main.rs:66-107`.

**Interfaces:** Consumes: `AppState`, `ui::serve`, new `scheduler::run`
signature. Produces: the running daemon.

- [ ] **Step 1: Replace the wiring**

```rust
    let cfg = Arc::new(cfg);
    let (config_tx, config_rx) = tokio::sync::watch::channel(cfg.clone());
    let (status_tx, status_rx) =
        tokio::sync::watch::channel(Arc::new(scheduler::StatusSnapshot {
            repos: Vec::new(),
            last_poll_finished_at: None,
            next_poll_at: None,
        }));

    match cfg.ui_auth_mode() {
        ui::AuthMode::Token => info!("ui auth: admin token login enabled"),
        ui::AuthMode::Proxy => info!("ui auth: trusting Remote-User from reverse proxy"),
        ui::AuthMode::Open => {
            if cfg.ui.bind_addr == "0.0.0.0" {
                tracing::warn!(
                    "ui is bound to 0.0.0.0 with NO authentication; \
                     ensure it is only reachable through your reverse proxy"
                );
            } else {
                info!("ui auth: open (no admin_token set)");
            }
        }
    }

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let ui_state = ui::AppState {
        config_rx: config_rx.clone(),
        status_rx,
        config_path: cfg.config_path.clone(),
        config_tx,
        sessions: Arc::new(ui::Sessions::default()),
        limiter: Arc::new(ui::LoginLimiter::default()),
        started_at: std::time::Instant::now(),
    };

    let ui_shutdown = shutdown_rx.clone();
    let ui_handle = tokio::spawn(async move {
        if let Err(e) = ui::serve(ui_state, ui_shutdown).await {
            error!("ui server exited with error: {e}");
        }
    });

    let scheduler_cfg = config_rx;
    let scheduler_handle = tokio::spawn(async move {
        if let Err(e) =
            scheduler::run(scheduler_cfg, github, state, status_tx, shutdown_rx).await
        {
            error!("scheduler exited with error: {e}");
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => info!("received SIGINT"),
        _ = unix_sigterm() => info!("received SIGTERM"),
    }

    let _ = shutdown_tx.send(true);
    let _ = ui_handle.await;
    let _ = scheduler_handle.await;
    info!("shutdown complete");
```

Add `use std::sync::Arc;` and `mod ui;` (`src/main.rs:1`).

`ui_auth_mode()` is a small helper on `Config` added in this task:

```rust
    pub fn ui_auth_mode(&self) -> AuthMode
```

Define `AuthMode` in `src/ui/auth.rs` (B4) and re-export it; to avoid a
`config → ui` dependency, instead define the three-way decision as a method
returning a plain enum declared in `config.rs`, and have `ui` re-use it. Resolve
whichever direction keeps the dependency graph clean (`config` must not depend
on `ui`).

The `Mailer` is no longer constructed in `main` (the scheduler builds it per
send), so remove `src/main.rs:82-88`.

- [ ] **Step 2: Verify the whole thing**

Run: `cargo build && cargo test && cargo clippy -- -D warnings`
Expected: ALL PASS, NO WARNINGS.

- [ ] **Step 3: Manual smoke test**

```bash
cp config.example.toml /tmp/test-config.toml
# set state_path = "/tmp/test-state.json", bind_addr = "127.0.0.1"
cargo run --release -- --config /tmp/test-config.toml
# in another shell:
curl -s localhost:8080/healthz
curl -s localhost:8080/api/config | head -c 400
curl -s -X PUT localhost:8080/api/config -H 'Content-Type: application/json' \
  -H 'X-Requested-With: gh-release-notify' \
  -d '{"poll_interval_seconds":7200,"sender":"bot@homelab.local","repos":["fosrl/pangolin"],"recipients":["you@example.com"],"smtp_host":"smtp.example.com","smtp_port":587,"smtp_encryption":"starttls","smtp_username":"postmaster"}'
# confirm comments survived:
grep -c '^#' /tmp/test-config.toml
# confirm the scheduler picked up the change without a restart:
# log line "config changed, recomputing next poll"
```

Clean up `/tmp/test-config.toml` and `/tmp/test-state.json`.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs src/config.rs
git commit -m "feat: wire UI server and config reload channels into main"
```

---

## Phase C — Deployment and documentation

### Task C1: Update `config.example.toml` and `.env.example`

**Files:** Modify `config.example.toml` (append), `.env.example` (append).

- [ ] **Step 1: Append the `[ui]` section** to `config.example.toml` with
  comments explaining: the three auth modes and that token+proxy is rejected,
  that `bind_addr`/`port` are not editable from the UI, that the container
  deployment sets `0.0.0.0`, and that `admin_token` may be supplied via
  `ADMIN_TOKEN` instead.

- [ ] **Step 2: Append `ADMIN_TOKEN`** to `.env.example` with a comment noting
  the alternative is `[ui].admin_token` in the config file and that leaving both
  unset means no login.

- [ ] **Step 3: Commit**

```bash
git add config.example.toml .env.example
git commit -m "docs: document ui section and ADMIN_TOKEN"
```

---

### Task C2: Update `docker-compose.yml`

**Files:** Modify `docker-compose.yml:14-21`.

- [ ] **Step 1: Make the config mount writable and expose loopback only**

Change `docker-compose.yml:15` to drop `:ro`, and add a `ports` entry after the
`volumes` block:

```yaml
    ports:
      - "127.0.0.1:8080:8080"
```

Add to `environment`:

```yaml
      - ADMIN_TOKEN=${ADMIN_TOKEN:-}
```

Add a comment above the volume noting that the config is mounted read-write so
the UI can save edits, and that mounting it `:ro` is a supported alternative
which disables saving in the UI.

- [ ] **Step 2: Commit**

```bash
git add docker-compose.yml
git commit -m "build: mount config read-write and expose UI on loopback"
```

---

### Task C3: Update `README.md`

**Files:** Modify `README.md`.

- [ ] **Step 1: Add a `## Web UI` section** covering: the URL and port, the
  three auth modes and that they are mutually exclusive, how to generate a token
  (`openssl rand -hex 32`) and where to put it, the reverse-proxy guidance
  (route to `gh-release-notify:8080` on `gh-release-notify-net`; for Authelia/
  Pangolin set `trust_proxy_auth = true` and note the spoofing caveat), the
  read-only-mount behaviour, the fact that config edits apply without a restart,
  and that manual file edits require a restart.

- [ ] **Step 2: Add a security-posture note** stating plainly: the UI can
  rewrite the file holding the SMTP password, so in a read-write container
  deployment token or proxy auth is effectively mandatory; `0.0.0.0` with no
  token logs a warning at startup.

- [ ] **Step 3: Add the third-party notice** for Pico CSS (MIT, version 2.1.1)
  in the License area.

- [ ] **Step 4: Add `cargo test ui`** to the focused-test list
  (`README.md:161-166`) and add `src/ui/` + `src/config_writeback.rs` to the
  Project layout block (`README.md:135-147`).

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: document web UI, auth modes, and security posture"
```

---

### Task C4: Update `AGENTS.md`

**Files:** Modify `AGENTS.md`.

- [ ] **Step 1: Amend the scope list**

In **Rules → Out of scope**, remove `web UI` and `HTTP health endpoint` from the
do-not-add list; leave everything else.

- [ ] **Step 2: Add a `## Web UI` section** recording the invariants:
  axum + `include_str!` single-file page, no JS build step; `watch<Arc<Config>>`
  reload with `send_replace`-based status publication; exactly one auth mode
  (token / proxy / open) and token+proxy is a startup error; the UI never
  returns or logs a secret; `bind_addr`/`port` are file/env-only and never
  UI-editable; config writes are `toml_edit` with validate-before-rename; the
  UI's only filesystem write target is the config path, never `state_path`.

- [ ] **Step 3: Update the Config & env section** with `[ui]`, `ADMIN_TOKEN`,
  and the dependency list (`axum`, `subtle`, `getrandom`, `toml_edit`).

- [ ] **Step 4: Note the post-implementation review** that the AGENTS.md change
  should be revisited with the `writing-for-agents` skill once implementation is
  complete (grilling decision).

- [ ] **Step 5: Commit**

```bash
git add AGENTS.md
git commit -m "docs: update scope rules and add web UI section"
```

---

### Task C5: Final verification gate

**Files:** None.

- [ ] **Step 1: Run the gate**

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
cargo build --release
```

Expected: ALL PASS, NO WARNINGS.

- [ ] **Step 2: Confirm untouched files**

Run: `git diff --stat HEAD~10 -- src/state.rs`
Expected: EMPTY. `state.rs` must not appear in this feature's diff.

- [ ] **Step 3: Container build (docker-first, podman-fallback)**

```bash
docker build -t gh-release-notify:test . || podman build -t gh-release-notify:test .
```

Expected: BUILD SUCCESS (probe for docker first, fall back to podman; if neither
exists, note it as not testable here). Verify the binary size increase is
consistent with the embedded assets (~71 kB plus the new deps).

- [ ] **Step 4: No commit** — this task is verification only. Its evidence goes
  in the task report.
