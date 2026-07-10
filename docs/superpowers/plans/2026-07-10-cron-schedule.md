# Cron Schedule Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional `cron_expression` config field that takes precedence over `poll_interval_seconds` to control poll timing via a standard 5-field cron expression evaluated in UTC.

**Architecture:** Add `cron = "0.17"` dependency. Extend `Config` with `cron_expression: Option<String>` (deserialized) and `cron_schedule: Option<cron::Schedule>` (populated by `validate()`). The scheduler's poll body stays unchanged; only the sleep section branches: cron mode computes the next occurrence via `schedule.after(&now).next()` and sleeps for the resulting `Duration`, interval mode is unchanged.

**Tech Stack:** Rust 2021, tokio (full), cron 0.17, chrono 0.4 (serde), serde, toml, anyhow, tracing.

## Global Constraints

- No comments in Rust source files unless explicitly requested. `config.example.toml`, `README.md`, and `AGENTS.md` are documentation files and MAY contain comments.
- No `unwrap`/`expect`/`panic` in non-test code. The existing `Regex::new(...).unwrap()` calls in `config.rs` and `expect("install SIGTERM handler")` in `main.rs` are deliberate plan-mandated exceptions — do not add new ones.
- All fallible operations return `Result`. Handle with `?` at task boundaries or log-and-continue.
- Conventional Commits: `<type>: <subject>`, subject <= 72 chars, imperative mood, no trailing period, no body.
- Verification gate before any commit: `cargo fmt && cargo clippy -- -D warnings && cargo test`.
- `cron` crate uses 6-field expressions (with seconds). 5-field user input is normalized by prepending `"0 "`.
- `cron` crate day-of-week: 1=Sunday .. 7=Saturday (not 0 or 7).
- `@`-prefixed shorthand (`@daily`, `@hourly`, etc.) is passed through to the `cron` parser directly, skipping the 5/6 field count check.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `Cargo.toml` | Modify | Add `cron = "0.17"` dependency |
| `src/config.rs` | Modify | Add `cron_expression` + `cron_schedule` fields, validation logic, unit tests |
| `src/scheduler.rs` | Modify | Add cron scheduling branch in `run()` sleep section |
| `src/main.rs` | Modify | Update startup log line to reflect active scheduling mode |
| `config.example.toml` | Modify | Document `cron_expression` field |
| `README.md` | Modify | Document `cron_expression` in config snippet and "What it does" section |
| `AGENTS.md` | Modify | Mention `cron_expression` in Config & env section |

---

### Task 1: Add `cron` dependency

**Files:**
- Modify: `Cargo.toml:6-18`

**Interfaces:**
- Consumes: nothing
- Produces: `cron` crate available for import as `cron::Schedule`

- [ ] **Step 1: Add the dependency**

In `Cargo.toml`, add `cron = "0.17"` after the `chrono` line (line 18) in the `[dependencies]` section:

```toml
chrono = { version = "0.4", features = ["serde"] }
cron = "0.17"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: BUILD SUCCESS — `cron` crate downloads and compiles.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add cron 0.17 dependency"
```

---

### Task 2: Add `cron_expression` and `cron_schedule` fields to `Config`

**Files:**
- Modify: `src/config.rs:1-13` (imports + struct)
- Test: `src/config.rs` (under `#[cfg(test)]`)

**Interfaces:**
- Consumes: `cron::Schedule` (from Task 1)
- Produces: `Config.cron_expression: Option<String>` and `Config.cron_schedule: Option<cron::Schedule>` fields available to `validate()`, `scheduler::run()`, and `main.rs`

- [ ] **Step 1: Write the failing test**

Add this test to the `tests` module in `src/config.rs` (after the `parses_valid_config` test, around line 133):

```rust
#[test]
fn parses_config_without_cron() {
    let dir = tempfile::tempdir().unwrap();
    let p = write_config(dir.path(), VALID);
    let cfg = Config::load(p.to_str().unwrap()).unwrap();
    assert!(cfg.cron_expression.is_none());
    assert!(cfg.cron_schedule.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test parses_config_without_cron`
Expected: COMPILE ERROR — `no field cron_expression on type Config` / `no field cron_schedule on type Config`

- [ ] **Step 3: Add the fields to `Config`**

In `src/config.rs`, update the imports (line 1-3) and the `Config` struct (lines 5-13):

Replace lines 1-13:

```rust
use anyhow::{anyhow, bail, Result};
use cron::Schedule;
use serde::Deserialize;
use std::env;
use std::str::FromStr;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub poll_interval_seconds: u64,
    pub state_path: String,
    pub sender: String,
    pub repos: Vec<String>,
    pub recipients: Vec<String>,
    pub smtp: SmtpConfig,
    #[serde(default)]
    pub cron_expression: Option<String>,
    #[serde(skip)]
    pub cron_schedule: Option<Schedule>,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test parses_config_without_cron`
Expected: PASS

- [ ] **Step 5: Run full test suite to verify no regressions**

Run: `cargo test`
Expected: ALL PASS (existing tests still pass — `cron_expression` defaults to `None` via `#[serde(default)]`)

- [ ] **Step 6: Commit**

```bash
git add src/config.rs
git commit -m "feat: add cron_expression and cron_schedule config fields"
```

---

### Task 3: Add cron validation logic to `Config::validate()`

**Files:**
- Modify: `src/config.rs:53-88` (validate method)
- Test: `src/config.rs` (under `#[cfg(test)]`)

**Interfaces:**
- Consumes: `Config.cron_expression: Option<String>` (from Task 2), `cron::Schedule::from_str` (from Task 1)
- Produces: `Config.cron_schedule: Option<Schedule>` populated after `validate()` runs; `Config::load()` returns a config with a parsed schedule if `cron_expression` was present and valid

- [ ] **Step 1: Write the failing tests**

Add these tests to the `tests` module in `src/config.rs` (after `parses_config_without_cron`):

```rust
#[test]
fn parses_valid_config_with_cron() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!("{VALID}cron_expression = \"0 */6 * * *\"\n");
    let p = write_config(dir.path(), &contents);
    let cfg = Config::load(p.to_str().unwrap()).unwrap();
    assert_eq!(cfg.cron_expression.as_deref(), Some("0 */6 * * *"));
    assert!(cfg.cron_schedule.is_some());
}

#[test]
fn rejects_invalid_cron_expression() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!("{VALID}cron_expression = \"not a cron\"\n");
    let p = write_config(dir.path(), &contents);
    let err = Config::load(p.to_str().unwrap()).unwrap_err();
    assert!(err.to_string().contains("cron_expression"));
}

#[test]
fn rejects_cron_wrong_field_count() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!("{VALID}cron_expression = \"0 9 *\"\n");
    let p = write_config(dir.path(), &contents);
    let err = Config::load(p.to_str().unwrap()).unwrap_err();
    assert!(err.to_string().contains("5 or 6 fields"));
}

#[test]
fn auto_prepends_seconds_for_5_fields() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!("{VALID}cron_expression = \"0 */6 * * *\"\n");
    let p = write_config(dir.path(), &contents);
    let cfg = Config::load(p.to_str().unwrap()).unwrap();
    assert!(cfg.cron_schedule.is_some());
}

#[test]
fn accepts_6_field_cron() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!("{VALID}cron_expression = \"0 0 */6 * * *\"\n");
    let p = write_config(dir.path(), &contents);
    let cfg = Config::load(p.to_str().unwrap()).unwrap();
    assert!(cfg.cron_schedule.is_some());
}

#[test]
fn accepts_cron_shorthand() {
    let dir = tempfile::tempdir().unwrap();
    let contents = format!("{VALID}cron_expression = \"@daily\"\n");
    let p = write_config(dir.path(), &contents);
    let cfg = Config::load(p.to_str().unwrap()).unwrap();
    assert!(cfg.cron_schedule.is_some());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test cron`
Expected: FAIL — `parses_valid_config_with_cron` fails because `cron_schedule` is `None` (validation doesn't populate it yet); `rejects_invalid_cron_expression` and `rejects_cron_wrong_field_count` fail because no validation error is raised.

- [ ] **Step 3: Implement the validation logic**

In `src/config.rs`, add cron validation at the end of the `validate()` method, just before the final `Ok(())` (after line 86, before line 87):

```rust
        if let Some(expr) = &self.cron_expression {
            let normalized = if expr.starts_with('@') {
                expr.clone()
            } else {
                let field_count = expr.split_whitespace().count();
                match field_count {
                    5 => format!("0 {expr}"),
                    6 => expr.clone(),
                    n => bail!("cron_expression must have 5 or 6 fields, got {n}"),
                }
            };
            let schedule = Schedule::from_str(&normalized)
                .map_err(|e| anyhow!("invalid cron_expression '{expr}': {e}"))?;
            self.cron_schedule = Some(schedule);
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test cron`
Expected: ALL 6 cron tests PASS

- [ ] **Step 5: Run full test suite**

Run: `cargo test`
Expected: ALL PASS

- [ ] **Step 6: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: NO WARNINGS

- [ ] **Step 7: Commit**

```bash
git add src/config.rs
git commit -m "feat: validate and parse cron_expression in config"
```

---

### Task 4: Add cron scheduling branch to `scheduler::run()`

**Files:**
- Modify: `src/scheduler.rs:1-4` (imports), `src/scheduler.rs:65-72` (sleep section)

**Interfaces:**
- Consumes: `Config.cron_schedule: Option<Schedule>` (from Task 3), `chrono::Utc`, `tokio::time::sleep`
- Produces: `scheduler::run()` uses cron schedule for sleep timing when `cron_schedule` is `Some`

- [ ] **Step 1: Update imports**

In `src/scheduler.rs`, replace lines 1-4:

```rust
use anyhow::Result;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{error, info, warn};
```

with:

```rust
use anyhow::Result;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{error, info, warn};

use chrono::Utc;
```

- [ ] **Step 2: Replace the sleep section**

In `src/scheduler.rs`, replace lines 65-72 (the `info!("tick complete...")` + `tokio::select!` block):

```rust
        info!("tick complete, sleeping {}s", cfg.poll_interval_seconds);
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => {
                info!("shutdown signaled, exiting scheduler");
                break;
            }
        }
```

with:

```rust
        if let Some(schedule) = &cfg.cron_schedule {
            let now = Utc::now();
            match schedule.after(&now).next() {
                Some(next_dt) => {
                    let duration = (next_dt - now).to_std().unwrap_or_default();
                    info!("tick complete, next poll at {next_dt}");
                    tokio::select! {
                        _ = tokio::time::sleep(duration) => {}
                        _ = shutdown.changed() => {
                            info!("shutdown signaled, exiting scheduler");
                            break;
                        }
                    }
                }
                None => {
                    error!(
                        "cron schedule has no future occurrences, \
                         falling back to poll_interval_seconds"
                    );
                    info!("tick complete, sleeping {}s", cfg.poll_interval_seconds);
                    tokio::select! {
                        _ = tokio::time::sleep(interval) => {}
                        _ = shutdown.changed() => {
                            info!("shutdown signaled, exiting scheduler");
                            break;
                        }
                    }
                }
            }
        } else {
            info!("tick complete, sleeping {}s", cfg.poll_interval_seconds);
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = shutdown.changed() => {
                    info!("shutdown signaled, exiting scheduler");
                    break;
                }
            }
        }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: BUILD SUCCESS

- [ ] **Step 4: Run full test suite**

Run: `cargo test`
Expected: ALL PASS (no scheduler tests exist; config tests unaffected)

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: NO WARNINGS

- [ ] **Step 6: Commit**

```bash
git add src/scheduler.rs
git commit -m "feat: add cron scheduling branch to scheduler"
```

---

### Task 5: Update startup log line in `main.rs`

**Files:**
- Modify: `src/main.rs:40-50`

**Interfaces:**
- Consumes: `Config.cron_schedule` and `Config.cron_expression` (from Task 3)
- Produces: Startup log reflects which scheduling mode is active

- [ ] **Step 1: Replace the log line**

In `src/main.rs`, replace lines 40-50:

```rust
    info!(
        "config loaded: {} repos, poll interval {}s, state path {}, token {}",
        cfg.repos.len(),
        cfg.poll_interval_seconds,
        cfg.state_path,
        if cfg.github_token().is_some() {
            "present"
        } else {
            "absent"
        }
    );
```

with:

```rust
    if cfg.cron_schedule.is_some() {
        info!(
            "config loaded: {} repos, cron schedule '{}', state path {}, token {}",
            cfg.repos.len(),
            cfg.cron_expression.as_deref().unwrap_or(""),
            cfg.state_path,
            if cfg.github_token().is_some() {
                "present"
            } else {
                "absent"
            }
        );
    } else {
        info!(
            "config loaded: {} repos, poll interval {}s, state path {}, token {}",
            cfg.repos.len(),
            cfg.poll_interval_seconds,
            cfg.state_path,
            if cfg.github_token().is_some() {
                "present"
            } else {
                "absent"
            }
        );
    }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: BUILD SUCCESS

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: ALL PASS

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: NO WARNINGS

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: log cron schedule or poll interval at startup"
```

---

### Task 6: Update `config.example.toml`

**Files:**
- Modify: `config.example.toml:1-2`

**Interfaces:**
- Consumes: nothing
- Produces: documented `cron_expression` field for users

- [ ] **Step 1: Add the cron_expression documentation**

In `config.example.toml`, replace lines 1-2:

```toml
# How often to poll GitHub, in seconds. Minimum 60.
poll_interval_seconds = 3600
```

with:

```toml
# How often to poll GitHub, in seconds. Minimum 60.
# Used as fallback when cron_expression is not set.
poll_interval_seconds = 3600

# Optional: cron expression to control poll timing. If present, takes
# precedence over poll_interval_seconds. Standard 5-field syntax:
#   minute hour day-of-month month day-of-week
# Examples:
#   "0 */6 * * *"      every 6 hours
#   "0 9 * * 1-5"      09:00 on weekdays
#   "*/15 * * * *"     every 15 minutes
# Timezone is UTC. Day-of-week: 1=Sunday .. 7=Saturday.
# Shorthand: @hourly, @daily, @weekly, @monthly, @yearly
# cron_expression = "0 */6 * * *"
```

- [ ] **Step 2: Commit**

```bash
git add config.example.toml
git commit -m "docs: document cron_expression in config example"
```

---

### Task 7: Update `README.md`

**Files:**
- Modify: `README.md:7` (What it does), `README.md:23-36` (config snippet)

**Interfaces:**
- Consumes: nothing
- Produces: user-facing documentation of the cron feature

- [ ] **Step 1: Update the "What it does" section**

In `README.md`, replace line 7:

```markdown
- Polls `GET /repos/{owner}/{repo}/releases/latest` for each configured repo at a configurable interval (default 1h).
```

with:

```markdown
- Polls `GET /repos/{owner}/{repo}/releases/latest` for each configured repo on a configurable schedule: a fixed interval (default 1h) or an optional cron expression for precise timing control.
```

- [ ] **Step 2: Update the config snippet**

In `README.md`, replace lines 23-36 (the toml code block):

```toml
poll_interval_seconds = 3600
state_path = "/state/state.json"   # /state/state.json for Docker, ./state.json for local runs
sender = "gh-release-notify@homelab.local"
repos = ["fosrl/pangolin", "fosrl/newt"]
recipients = ["you@example.com"]

[smtp]
host = "smtp.example.com"
port = 587
encryption = "starttls"            # "starttls" (587) | "tls" (465) | "none" (25)
username = "postmaster@example.com"
password = "changeme"                # prefer SMTP_PASSWORD env var instead
```

with:

```toml
poll_interval_seconds = 3600
state_path = "/state/state.json"   # /state/state.json for Docker, ./state.json for local runs
sender = "gh-release-notify@homelab.local"
repos = ["fosrl/pangolin", "fosrl/newt"]
recipients = ["you@example.com"]

# Optional: cron expression (takes precedence over poll_interval_seconds).
# Standard 5-field: minute hour day-of-month month day-of-week.
# Timezone is UTC. Day-of-week: 1=Sunday .. 7=Saturday.
# cron_expression = "0 */6 * * *"

[smtp]
host = "smtp.example.com"
port = 587
encryption = "starttls"            # "starttls" (587) | "tls" (465) | "none" (25)
username = "postmaster@example.com"
password = "changeme"                # prefer SMTP_PASSWORD env var instead
```

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: document cron_expression in README"
```

---

### Task 8: Update `AGENTS.md`

**Files:**
- Modify: `AGENTS.md` (Config & env section)

**Interfaces:**
- Consumes: nothing
- Produces: agent-facing documentation of the cron feature

- [ ] **Step 1: Add cron_expression to the Config & env section**

In `AGENTS.md`, find the "Config & env" section (the bullet list starting with "Config file: TOML..."). Add a new bullet after the `RUST_LOG` bullet:

```markdown
- Optional `cron_expression` in config: standard 5-field cron expression (UTC) that takes precedence over `poll_interval_seconds` when present. Auto-prepends seconds field for the `cron` crate. Day-of-week: 1=Sunday .. 7=Saturday.
```

- [ ] **Step 2: Commit**

```bash
git add AGENTS.md
git commit -m "docs: document cron_expression in AGENTS.md"
```

---

### Task 9: Final verification gate

**Files:**
- None (verification only)

- [ ] **Step 1: Run the full verification gate**

Run:
```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
Expected: ALL PASS, NO WARNINGS

- [ ] **Step 2: Verify release build**

Run: `cargo build --release`
Expected: BUILD SUCCESS

- [ ] **Step 3: Manual smoke test (optional, if environment allows)**

Create a test config with:
```toml
poll_interval_seconds = 60
cron_expression = "*/2 * * * *"
```
Run: `cargo run -- --config ./test_config.toml`
Expected: Logs show `"config loaded: ... cron schedule '*/2 * * * *' ..."`, polls happen at 2-minute cron boundaries (not every 60 seconds), log line `"tick complete, next poll at ..."` shows the next UTC time.

Clean up: `rm -f test_config.toml state.json`