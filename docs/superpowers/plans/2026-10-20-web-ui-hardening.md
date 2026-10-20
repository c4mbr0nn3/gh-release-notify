# Web UI Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the shipped Docker deployment reachable and saveable, make config writes crash-safe and race-free, and make the login limiter actually limit.

**Architecture:** Five independent defects from the `feat/web-ui` review, fixed in dependency order: config gains real env overrides for the UI bind/port (the docs already claim they exist), the compose deployment mounts the config *directory* instead of the file (a file bind mount can never be atomically renamed onto), the write-back path writes a private, fsynced, uniquely named tmp file and preserves the target's mode, the UI serialises concurrent saves and stops probing with the save temp file, and the login limiter keys on the socket peer (or the proxy-appended `X-Forwarded-For` entry) with stale-entry purging.

**Tech Stack:** Rust 2021, tokio, axum 0.8, toml_edit, tempfile (dev), docker compose (probe `docker compose` first, then `podman compose`/`docker-compose`).

**Spec:** `docs/SPEC-web-ui.md`, `docs/SPEC-config-writeback.md`, `AGENTS.md`

## Global Constraints

- Rust edition 2021. No comments in Rust source files. No `unwrap`/`expect`/`panic` in non-test code.
- All fallible operations return `Result`; map I/O errors to `SaveError` variants, never swallow them.
- The verification gate must pass before every commit: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`. Also run `cargo clippy --all-targets -- -D warnings` before the final commit.
- Commit convention: Conventional Commits, subject only, <= 72 chars, no body.
- `state_path` and `ui.bind_addr`/`ui.port` remain read-only from the UI; the new env overrides are startup-time only and are never written back to the config file.
- Empty env override values are startup errors, matching the existing `STATE_PATH` rule.
- Container tooling is docker-first with podman fallback; if neither is present, report the compose check as not testable rather than skipping silently.
- Do not add retry/backoff, DB-backed state, per-repo overrides, or HTML email.

## Review Focus

- **Saving through a single-file bind mount** — an operator who edits the config from the UI in Docker expects a 200 and a changed file, not `Device or resource busy`. The mount contract and the error text must both name the fix.
- **Two saves in flight** (double-click, two tabs) — a reasonable person expects the last one to win, not an empty config file that stops the daemon from starting.
- **File mode after a save** — an operator who chmodded the config to 0600 expects the SMTP password and admin token to stay unreadable to other users.
- **Login attempts from many addresses or a spoofed `X-Forwarded-For`** — the limiter must slow an attacker down and must not let one client lock out everyone else.
- **`UI_BIND_ADDR`/`UI_PORT` set to garbage or empty** — startup must fail naming the setting, not silently bind the wrong interface or port. (A garbage bind address fails inside `validate()` with `ui.bind_addr ... is not a valid IP address`; that names the field, not the env var.)

---

### Task 1: UI bind/port env overrides

**Files:**
- Modify: `src/config.rs:84-93` (`Config::load`), add `apply_ui_env_overrides` beside `Config::load`
- Test: `src/config.rs` `mod tests` (reuse `ENV_LOCK`, `write_config`, `VALID`)

**Interfaces:**
- Consumes: `Config { ui: UiConfig { bind_addr: String, port: u16, .. }, .. }`
- Produces: `fn apply_ui_env_overrides(cfg: &mut Config) -> Result<()>`; `Config::load` applies it after TOML parse and before `validate()`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn ui_bind_addr_env_override_wins_over_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    env::remove_var("UI_PORT");
    env::set_var("UI_BIND_ADDR", "0.0.0.0");
    let dir = tempfile::tempdir().unwrap();
    let p = write_config(dir.path(), VALID);
    let cfg = Config::load(p.to_str().unwrap()).unwrap();
    env::remove_var("UI_BIND_ADDR");
    assert_eq!(cfg.ui.bind_addr, "0.0.0.0");
}

#[test]
fn ui_port_env_override_wins_over_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    env::remove_var("UI_BIND_ADDR");
    env::set_var("UI_PORT", "9090");
    let dir = tempfile::tempdir().unwrap();
    let p = write_config(dir.path(), VALID);
    let cfg = Config::load(p.to_str().unwrap()).unwrap();
    env::remove_var("UI_PORT");
    assert_eq!(cfg.ui.port, 9090);
}

#[test]
fn empty_or_invalid_ui_env_override_is_a_startup_error() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let p = write_config(dir.path(), VALID);
    let path = p.to_str().unwrap();

    env::set_var("UI_BIND_ADDR", "");
    let err = Config::load(path).unwrap_err();
    env::remove_var("UI_BIND_ADDR");
    assert!(err.to_string().contains("UI_BIND_ADDR"), "{err}");

    env::set_var("UI_PORT", "not-a-port");
    let err = Config::load(path).unwrap_err();
    env::remove_var("UI_PORT");
    assert!(err.to_string().contains("UI_PORT"), "{err}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test config::tests`
Expected: FAIL — `bind_addr` is still `127.0.0.1` from the file / no error is raised.

- [ ] **Step 3: Implement `apply_ui_env_overrides` and call it from `Config::load`**

In `Config::load`, after `cfg.config_path = path.to_string();` and before `cfg.validate()?;`, add `apply_ui_env_overrides(&mut cfg)?;`.

```rust
fn apply_ui_env_overrides(cfg: &mut Config) -> Result<()> {
    match env::var("UI_BIND_ADDR") {
        Ok(v) if v.trim().is_empty() => bail!("UI_BIND_ADDR is set but empty"),
        Ok(v) => cfg.ui.bind_addr = v.trim().to_string(),
        Err(_) => {}
    }
    match env::var("UI_PORT") {
        Ok(v) if v.trim().is_empty() => bail!("UI_PORT is set but empty"),
        Ok(v) => {
            cfg.ui.port = v
                .trim()
                .parse::<u16>()
                .map_err(|e| anyhow!("UI_PORT '{}' is not a valid port: {e}", v.trim()))?
        }
        Err(_) => {}
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test config::tests`
Expected: PASS (3 new UI tests plus the existing `config` suite).

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat: allow UI bind and port overrides from env"
```

---

### Task 2: Fix the shipped deployment

**Files:**
- Modify: `docker-compose.yml:14-27`
- Modify: `config.example.toml:54-60`
- Modify: `README.md:146`
- Modify: `docs/SPEC-web-ui.md:334`

**Interfaces:**
- Consumes: `UI_BIND_ADDR`, `UI_PORT`, `CONFIG_PATH` from Task 1
- Produces: a compose service whose config is a directory mount and whose UI binds `0.0.0.0` inside the container

- [ ] **Step 1: Change the compose mounts and environment**

Replace the volumes block and add `UI_BIND_ADDR`:

```yaml
    # The config DIRECTORY is mounted read-write so the web UI can save edits.
    # Mounting a single file (`./config.toml:/config/config.toml`) cannot work:
    # the UI saves with tmp+rename, and renaming onto a bind-mounted file fails
    # with EBUSY. Mount it `:ro` instead to make the file immutable; the UI then
    # reports it as not writable and disables saving.
    volumes:
      - ./config:/config
      - ./state:/state
```

```yaml
      - CONFIG_PATH=/config/config.toml
      - UI_BIND_ADDR=0.0.0.0
```

- [ ] **Step 2: Make the docs true**

- `config.example.toml:54-60`: keep `bind_addr = "127.0.0.1"` as the local default and correct the comment to say the shipped compose file sets `UI_BIND_ADDR=0.0.0.0` for the container (env override, not the file).
- `README.md:44-53` (env table): add `UI_BIND_ADDR` and `UI_PORT` rows, saying they override `[ui].bind_addr`/`[ui].port` at startup and are never written back.
- `README.md:146`: say compose sets `UI_BIND_ADDR=0.0.0.0` and mounts `./config` as a directory, add one sentence that a single-file bind mount cannot be saved from the UI, and add a migration line: existing deployments must `mkdir config && mv config.toml config/config.toml` and `chown 10001:10001 config` before pulling the new compose file (the container user needs write access, exactly like `./state` in `README.md:107-113`).
- `README.md:148-150` (read-only config section): retire the single-file `:ro` example. Document `./config:/config:ro` (directory) as the hardening variant; note that a single-file `:ro` mount no longer yields the "not writable" banner, because the probe tests the writable parent directory, and that its saves fail with the bind-mount error instead.
- `README.md` (Web UI section): one sentence that the page shows the file's `bind_addr`/`port`, not an env override, so a container deployment reads `127.0.0.1` there while the process binds `0.0.0.0`.
- `docs/SPEC-web-ui.md:72-73`: change "Env override: No" to name `UI_BIND_ADDR`/`UI_PORT` for both rows.
- `docs/SPEC-web-ui.md:334`: same correction where it claims compose sets `bind_addr`.
- `docs/SPEC-config-writeback.md:201-203`: record that `SAVE_LOCK` in `put_config` now serialises read-modify-write, so concurrent saves cannot lose an edit; last-writer-wins still describes the order of the two outcomes.

- [ ] **Step 3: Verify the compose file parses**

Run: `docker compose config >/dev/null && echo COMPOSE_OK` (fallback: `podman compose config` or `docker-compose config`). If none is installed, record "not testable here" and continue.

- [ ] **Step 4: Verify the mount contract against a real container filesystem**

Run the daemon locally against a directory-mounted config and save via the API; confirm `PUT /api/config` returns 200 and the file changes:

```bash
mkdir -p /tmp/grn-cfg && cp config.example.toml /tmp/grn-cfg/config.toml
# edit /tmp/grn-cfg/config.toml: state_path, ui.port = 18090, no admin_token
cargo run -- --config /tmp/grn-cfg/config.toml &
curl -s http://127.0.0.1:18090/api/config | head -c 200
curl -s -X PUT http://127.0.0.1:18090/api/config \
  -H 'Content-Type: application/json' -H 'X-Requested-With: gh-release-notify' \
  -d '{"poll_interval_seconds":3600,"sender":"bot@homelab.local","sender_name":"Bot","repos":["fosrl/pangolin"],"recipients":["you@example.com"],"smtp_host":"smtp.example.com","smtp_port":587,"smtp_encryption":"starttls","smtp_username":"postmaster","admin_token":null}'
```

Expected: `{"ok":true}` and `/tmp/grn-cfg/config.toml` updated. Kill the daemon afterwards.

- [ ] **Step 5: Commit**

```bash
git add docker-compose.yml config.example.toml README.md docs/SPEC-web-ui.md
git commit -m "fix: mount config directory and bind UI in container"
```

---

### Task 3: Crash-safe, private, uniquely named config writes

**Files:**
- Modify: `src/config_writeback.rs:108-128` (`apply`)
- Test: `src/config_writeback.rs` `mod tests`

**Interfaces:**
- Consumes: `apply(path, edit)` existing flow; `SaveError::{Io, Unwritable, Invalid}`
- Produces:
  - `fn write_private(path: &str, contents: &str) -> std::io::Result<()>` — creates with mode 0600 on unix, writes all, `sync_all()`
  - `fn unique_tmp_path(path: &str) -> String` — `format!("{path}.tmp.{pid}.{seq}")` with `static TMP_SEQ: AtomicU64`
  - `fn copy_mode(from: &str, to: &str)` — defined unconditionally; on unix it copies the source mode best-effort, on non-unix it is a no-op body
  - `use std::io::Write;` at the top of the file (for `write_all`)

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(unix)]
#[test]
fn save_preserves_file_mode() {
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test config_writeback::tests::save_`
Expected: `save_preserves_file_mode` FAILs under the usual umask 022 (the tmp gets 0644). Under umask 077 the old code also produces 0600, so the test passes for the wrong reason; run it under `umask 022` if it passes on the first try.

- [ ] **Step 3: Implement the three helpers and use them in `apply`**

Replace `let tmp = format!("{path}.tmp"); std::fs::write(...); std::fs::rename(...)` with:

```rust
let tmp = unique_tmp_path(path);
write_private(&tmp, &rendered)
    .map_err(|e| classify(&e, &format!("failed to write {tmp}")))?;
copy_mode(path, &tmp);
if let Err(e) = std::fs::rename(&tmp, path) {
    let _ = std::fs::remove_file(&tmp);
    return Err(classify(&e, &format!("failed to rename {tmp} -> {path}")));
}
```

`write_private` must use `std::fs::OpenOptions` with `create(true).truncate(true).write(true)`, `#[cfg(unix)]` `use std::os::unix::fs::OpenOptionsExt` and `.mode(0o600)`, then `write_all` and `sync_all`.

- [ ] **Step 4: Run the full config_writeback suite**

Run: `cargo test config_writeback`
Expected: PASS, including the four `sender_name` tests.

- [ ] **Step 5: Commit**

```bash
git add src/config_writeback.rs
git commit -m "fix: preserve config mode and sync writes before rename"
```

---

### Task 4: Name the bind-mount failure

**Files:**
- Modify: `src/config_writeback.rs` `classify` (line ~130)
- Test: `src/config_writeback.rs` `mod tests`

**Interfaces:**
- Consumes: `classify(&std::io::Error, ctx: &str) -> SaveError`
- Produces: `SaveError::Unwritable` with an actionable message for `raw_os_error() == Some(16)` (EBUSY) on unix

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test config_writeback::tests::rename_onto_mount_point_reports_mount_cause`
Expected: FAIL — the error maps to `SaveError::Io`, not `Unwritable`.

- [ ] **Step 3: Extend `classify`**

Before the `kind()` match, add:

```rust
#[cfg(unix)]
if e.raw_os_error() == Some(16) {
    return SaveError::Unwritable(format!(
        "{ctx}: the config path appears to be a bind-mounted file; mount its parent directory instead"
    ));
}
```

- [ ] **Step 4: Verify**

Run: `cargo test config_writeback` and then `cargo clippy -- -D warnings`.
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/config_writeback.rs
git commit -m "fix: explain bind-mounted config save failure"
```

---

### Task 5: Serialise saves and stop probing with the save temp file

**Files:**
- Modify: `src/ui/mod.rs:196-210` (`probe_config_writable`, `put_config`)
- Test: `src/ui/mod.rs` `mod tests`

**Interfaces:**
- Consumes: `crate::config_writeback::apply`, `AppState::config_path`
- Produces: `static SAVE_LOCK: tokio::sync::Mutex<()>`; `static PROBE_SEQ: std::sync::atomic::AtomicU64`; `probe_config_writable(&str) -> bool` creates and removes its own uniquely named sidecar, never `{path}.tmp`; `put_config` holds `SAVE_LOCK` across its `spawn_blocking`

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn probe_survives_a_directory_squatting_on_the_old_tmp_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_test_config(dir.path());
    let squatter = format!("{}.tmp", path.display());
    std::fs::create_dir(&squatter).unwrap();
    assert!(
        probe_config_writable(path.to_str().unwrap()),
        "probe must not be blocked by an entry named like the old save temp file"
    );
    let entries: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".probe."))
        .collect();
    assert!(entries.is_empty(), "probe left artefacts: {entries:?}");
}
```

This is the honest form of the race: the old probe opens `{path}.tmp` for writing, gets `EISDIR` because a directory now occupies that name, and returns `false`; the new probe uses its own `{path}.probe.{pid}.{seq}` name and succeeds.

Add the helper `write_test_config(dir: &std::path::Path) -> std::path::PathBuf`, next to the existing `test_config`: write the same body as `src/config.rs`'s `VALID` constant **including `password = "secret"`** (`validate()` requires a password whenever `smtp.username` is set, `config.rs:176-178`), with `state_path` pointing at `dir/state.json`, `repos = ["fosrl/pangolin"]`, one recipient, and `[ui] port = 18090`, to `dir/config.toml`, then return the path.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test ui::tests::probe_survives`
Expected: FAIL — the old probe cannot open the directory named `config.toml.tmp` and returns `false`.

- [ ] **Step 3: Rewrite the probe and serialise `put_config`**

```rust
static PROBE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn probe_config_writable(path: &str) -> bool {
    let probe = format!(
        "{path}.probe.{}.{}",
        std::process::id(),
        PROBE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}
```

The probe must not open the config file itself: a read-only file inside a writable directory is still replaceable by `rename`, so opening the target would report "not writable" for a save that would succeed.

```rust
static SAVE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
```

At the top of `put_config`, before cloning the path:

```rust
let _save = SAVE_LOCK.lock().await;
```

- [ ] **Step 4: Add the concurrency test**

```rust
#[tokio::test]
async fn concurrent_saves_do_not_lose_an_edit() {
    let _guard = lock_env();
    let dir = tempfile::tempdir().unwrap();
    let path = write_test_config(dir.path());
    let state = test_state(path.to_str().unwrap());
    let app = router(state);
    let body_a = r#"{"poll_interval_seconds":7200,"sender":"bot@homelab.local","sender_name":null,"repos":["fosrl/pangolin"],"recipients":["you@example.com"],"smtp_host":"smtp.example.com","smtp_port":587,"smtp_encryption":"starttls","smtp_username":"postmaster","admin_token":"tokA"}"#;
    let body_b = r#"{"poll_interval_seconds":3600,"sender":"other@homelab.local","sender_name":"Other","repos":["fosrl/pangolin"],"recipients":["you@example.com"],"smtp_host":"smtp.example.com","smtp_port":587,"smtp_encryption":"starttls","smtp_username":"postmaster"}"#;
    let (a, b) = tokio::join!(
        app.clone().oneshot(put_req(body_a)),
        app.clone().oneshot(put_req(body_b)),
    );
    assert_eq!(a.unwrap().status(), StatusCode::OK);
    assert_eq!(b.unwrap().status(), StatusCode::OK);
    let cfg = crate::config::Config::load(path.to_str().unwrap()).unwrap();
    assert_eq!(
        cfg.ui.admin_token, "tokA",
        "the admin token written by one save was dropped by the other"
    );
    assert!(
        (cfg.poll_interval_seconds == 7200 && cfg.sender == "bot@homelab.local")
            || (cfg.poll_interval_seconds == 3600 && cfg.sender == "other@homelab.local"),
        "config mixed two writers: poll={} sender={}",
        cfg.poll_interval_seconds,
        cfg.sender
    );
}
```

Why this fails without the lock: `apply` is read-modify-write. `body_b` omits `admin_token`, so it does not write that key — it preserves whatever its own read saw. If `b`'s read happens before `a`'s rename and `b`'s rename lands after it, `b`'s render (read with no admin token) overwrites `a`'s, and `cfg.ui.admin_token` is empty. With `SAVE_LOCK`, the second save's read always follows the first save's rename, so `tokA` is preserved and the field pair is exactly one body's. The two cross pairs are impossible in either world because one `apply` writes poll, sender and sender_name in a single render; that is why only the two single-writer pairs are accepted.

`SAVE_LOCK` supersedes the "no lock, last writer wins" note in `docs/SPEC-config-writeback.md:201-203`; add a sentence to Task 2's doc step recording that the lock now prevents lost updates and that last-writer-wins still describes the outcome order.

Define the three helpers next to the existing `app()` test helper:

```rust
fn test_state(path: &str) -> AppState {
    let cfg = Arc::new(crate::config::Config::load(path).unwrap());
    let (config_tx, config_rx) = tokio::sync::watch::channel(cfg);
    let (status_tx, status_rx) = tokio::sync::watch::channel(Arc::new(
        crate::scheduler::StatusSnapshot { repos: Vec::new(), last_poll_finished_at: None, next_poll_at: None },
    ));
    let _ = status_tx;
    AppState {
        config_rx,
        config_tx,
        status_rx,
        config_path: path.to_string(),
        sessions: Arc::new(Sessions::default()),
        limiter: Arc::new(LoginLimiter::default()),
        started_at: Instant::now(),
    }
}

fn put_req(body: &'static str) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri("/api/config")
        .header("content-type", "application/json")
        .header("x-requested-with", "gh-release-notify")
        .body(Body::from(body))
        .unwrap()
}
```

- [ ] **Step 5: Run the UI suite**

Run: `cargo test ui::tests` and `cargo test config_writeback`.
Expected: PASS. `concurrent_saves_do_not_lose_an_edit` asserts a property that only holds with serialisation (the omitted key in `body_b` cannot clobber the key written by `body_a`); if it fails, the lock is not covering the read-modify-write, or the write path lost atomicity. It is a race, so it may pass by luck a few times before failing; run it repeatedly (`for i in 1 2 3 4 5; do cargo test concurrent_saves -- --exact ui::tests::concurrent_saves_do_not_lose_an_edit; done`) and confirm the lock is held across `spawn_blocking`.

- [ ] **Step 6: Commit**

```bash
git add src/ui/mod.rs
git commit -m "fix: serialise config saves and isolate writability probe"
```

---

### Task 6: Key the login limiter on the real client

**Files:**
- Modify: `src/ui/auth.rs:66-90` (`LoginLimiter`)
- Modify: `src/ui/mod.rs:38-45` (`router` unchanged), `:108-140` (`login`), `:55-67` (`serve`)
- Test: `src/ui/auth.rs` new `mod tests`; `src/ui/mod.rs` `mod tests`

**Interfaces:**
- Consumes: `Config::ui.trust_proxy_auth: bool`, `HeaderMap`, `std::net::SocketAddr`
- Produces:
  - `pub fn client_key(trust_proxy: bool, headers: &HeaderMap, peer: Option<SocketAddr>) -> String`
  - `LoginLimiter::allow(&self, key: &str) -> bool` now purges entries older than the window on every call
  - `LoginLimiter::reset(&self, key: &str)` unchanged
  - `serve()` uses `into_make_service_with_connect_info::<SocketAddr>()`
  - `login()` extracts `Option<axum::Extension<axum::extract::ConnectInfo<SocketAddr>>>` (verified: `Extension<T>` is the only wrapper with the blanket `OptionalFromRequestParts` impl in axum 0.8.9; `Option<ConnectInfo<T>>` does not compile, and a bare `ConnectInfo<T>` rejects extension-less `oneshot` requests with a 500, breaking the existing login tests)

- [ ] **Step 1: Write the failing tests**

In `src/ui/auth.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::SocketAddr;

    #[test]
    fn limiter_blocks_sixth_attempt_and_resets() {
        let l = LoginLimiter::default();
        for _ in 0..5 {
            assert!(l.allow("1.2.3.4"));
        }
        assert!(!l.allow("1.2.3.4"));
        l.reset("1.2.3.4");
        assert!(l.allow("1.2.3.4"));
    }

    #[test]
    fn limiter_purges_stale_keys() {
        let l = LoginLimiter::default();
        for i in 0..200 {
            l.allow(&format!("10.0.0.{i}"));
        }
        l.expire_all_for_test();
        l.allow("10.0.0.1");
        assert_eq!(l.tracked(), 1);
    }

    #[test]
    fn client_key_uses_socket_when_not_behind_proxy() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6"));
        let peer: SocketAddr = "10.0.0.9:5555".parse().unwrap();
        assert_eq!(client_key(false, &h, Some(peer)), "10.0.0.9");
    }

    #[test]
    fn client_key_uses_last_forwarded_entry_behind_proxy() {
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("6.6.6.6, 203.0.113.7"),
        );
        let peer: SocketAddr = "10.0.0.9:5555".parse().unwrap();
        assert_eq!(client_key(true, &h, Some(peer)), "203.0.113.7");
    }

    #[test]
    fn client_key_falls_back_to_peer_without_forwarded_header() {
        let h = HeaderMap::new();
        let peer: SocketAddr = "10.0.0.9:5555".parse().unwrap();
        assert_eq!(client_key(true, &h, Some(peer)), "10.0.0.9");
    }
}
```

In `src/ui/mod.rs` add two handler tests in token mode. Write a config that includes `[ui] admin_token = "secret"` (add a `token: bool` variant to `write_test_config`, or a second helper `write_token_config`), build the router from it, and POST `/login` with `content-type: application/json`; `/login` is a public route, so no CSRF header is needed.

- `login_rate_limit_blocks_after_five_failures`: five wrong-token posts return `401`, the sixth returns `429`.
- `successful_login_resets_the_bucket`: four wrong-token posts return `401`, the fifth post with the correct token returns `200`, and one more wrong-token post returns `401` (a fresh bucket).

Do not put the correct login after the `429`: the limiter counts before it checks the token (`auth.rs:67-79`), and `reset` only runs after a permitted attempt succeeds, so a sixth-then-correct sequence can only ever return `429`.

Both tests take the existing `lock_env()` guard as their first line, like every other handler test (`mod.rs:331`): otherwise an ambient `ADMIN_TOKEN` resolves `cfg.admin_token()` away from the file's `"secret"` and the tests fail for the wrong reason.

The handler must tolerate the absent connect-info extension: `Option<Extension<ConnectInfo<SocketAddr>>>` gives `None` under `oneshot`, so the limiter falls back to the `"unknown"` key and the window still applies.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test ui::auth::tests`
Expected: FAIL — `client_key`, `tracked`, `expire_all_for_test` do not exist; the limiter never purges.

- [ ] **Step 3: Implement**

In `auth.rs`:

```rust
pub fn client_key(
    trust_proxy: bool,
    headers: &HeaderMap,
    peer: Option<std::net::SocketAddr>,
) -> String {
    if trust_proxy {
        if let Some(last) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next_back())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            return last.to_string();
        }
    }
    match peer {
        Some(addr) => addr.ip().to_string(),
        None => "unknown".to_string(),
    }
}
```

In `LoginLimiter::allow`, purge before the entry lookup:

```rust
m.retain(|_, (_, ts)| now - *ts <= Duration::minutes(1));
```

Add test-only accessors:

```rust
#[cfg(test)]
pub(crate) fn tracked(&self) -> usize {
    self.attempts.lock().map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
pub(crate) fn expire_all_for_test(&self) {
    if let Ok(mut m) = self.attempts.lock() {
        for entry in m.values_mut() {
            entry.1 = Utc::now() - Duration::minutes(5);
        }
    }
}
```

In `mod.rs`:

```rust
async fn login(
    State(state): State<AppState>,
    connect: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<LoginBody>,
) -> Response {
    let trust_proxy = state.config_rx.borrow().ui.trust_proxy_auth;
    let peer = connect.map(|axum::Extension(c)| c.0);
    let key = auth::client_key(trust_proxy, &headers, peer);
    ...
}
```

and in `serve()`:

```rust
axum::serve(
    listener,
    router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
)
```

Only the service argument changes: keep the existing `.with_graceful_shutdown(async move { ... })` chain from `src/ui/mod.rs:59-65` exactly as it is, or shutdown regresses silently.

`Option<Extension<ConnectInfo<_>>>` yields `None` under `tower::ServiceExt::oneshot` (no connect-info extension in the test requests) and `Some` under `into_make_service_with_connect_info`, so the existing login tests keep passing without changes.

- [ ] **Step 4: Verify**

Run: `cargo test ui::` and then `cargo clippy --all-targets -- -D warnings`.
Expected: PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
git add src/ui/auth.rs src/ui/mod.rs
git commit -m "fix: key login limiter on socket peer and purge stale keys"
```

---

### Task 7: Empty `SMTP_PASSWORD` is not an override

**Files:**
- Modify: `src/config.rs:92-95` (`smtp_password`)
- Modify: `src/ui/mod.rs:181-184` (`env_managed.smtp_password`)
- Test: `src/config.rs` `mod tests`

**Interfaces:**
- Consumes: `Config::smtp_password(&self) -> String`, env `SMTP_PASSWORD`
- Produces: `smtp_password()` skips an empty env value (matches `admin_token()` and `github_token()`); `env_managed.smtp_password` is true only when the env value is non-empty

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn empty_smtp_password_env_does_not_override_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    env::set_var("SMTP_PASSWORD", "");
    let dir = tempfile::tempdir().unwrap();
    let p = write_config(dir.path(), VALID);
    let cfg = Config::load(p.to_str().unwrap()).unwrap();
    env::remove_var("SMTP_PASSWORD");
    assert_eq!(cfg.smtp_password(), "secret");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test config::tests::empty_smtp_password_env`
Expected: FAIL — `smtp_password()` returns the empty string.

- [ ] **Step 3: Implement**

```rust
pub fn smtp_password(&self) -> String {
    match env::var("SMTP_PASSWORD") {
        Ok(v) if !v.is_empty() => v,
        _ => self.smtp.password.clone(),
    }
}
```

In `env_managed`, change `std::env::var("SMTP_PASSWORD").is_ok()` to:

```rust
std::env::var("SMTP_PASSWORD").is_ok_and(|v| !v.is_empty()),
```

- [ ] **Step 4: Verify**

Run: `cargo test config::tests`, `cargo test ui::tests`, `cargo clippy -- -D warnings`.
Expected: PASS. This matters because `docker-compose.yml:24` passes `${SMTP_PASSWORD:-}`, which is an empty string when the operator keeps the password in the file; before this change that fails startup validation.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/ui/mod.rs
git commit -m "fix: ignore empty SMTP_PASSWORD env override"
```

---

### Task 8: Final gate and manual verification

**Files:** none (verification only)

- [ ] **Step 1: Run the full gate**

```bash
cargo fmt
cargo clippy -- -D warnings
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

Expected: all pass, 95+ tests.

- [ ] **Step 2: Verify the four review findings are gone**

- Directory-mounted config saves through `PUT /api/config` (Task 2, Step 4) — 200 and file changed.
- Container UI reachable: `UI_BIND_ADDR=0.0.0.0` set in compose and honored by `Config::load` (Task 1 + 2).
- Two concurrent `PUT`s leave a parseable config (Task 5 test).
- A 0600 config stays 0600 after save (Task 3 test).
- Spoofed leftmost `X-Forwarded-For` cannot mint new limiter buckets behind a trusted proxy (Task 6 test).

- [ ] **Step 3: Update AGENTS.md if behavior changed**

Add one line under "Config & env": `UI_BIND_ADDR` and `UI_PORT` override `[ui].bind_addr`/`[ui].port` at startup and are never written back; the compose file sets `UI_BIND_ADDR=0.0.0.0`.

```bash
git add AGENTS.md
git commit -m "docs: document UI env overrides"
```
