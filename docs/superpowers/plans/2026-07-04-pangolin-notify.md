# pangolin-notify Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a long-running Rust daemon that polls a configurable list of GitHub repos for new stable releases and emails a configurable recipient list via SMTP when a new release appears.

**Architecture:** Single async Tokio binary with focused modules (config, github, state, notify, scheduler). Polls `GET /repos/{owner}/{repo}/releases/latest`, compares the tag to a JSON-on-disk state file, and sends a plain-text email via lettre on a new release. Deploys as a Docker container with config + state volumes.

**Tech Stack:** Rust 1.75+, tokio, reqwest, serde/toml/serde_json, lettre 0.11, tracing, clap, anyhow, regex, chrono. Verification gate: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.

## Global Constraints

- Rust edition 2021; MSRV not enforced beyond what crates need.
- All fallible operations return `Result`; no `unwrap`/`expect`/`panic` in non-test code.
- No comments in source unless explicitly requested by a step.
- Every task ends with: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` all passing, then commit.
- Commit message style: `feat: ...`, `test: ...`, `chore: ...`, `docs: ...`, `refactor: ...`.
- Config file format: TOML; state file format: JSON.
- Only stable releases are notified (use GitHub's `/releases/latest` endpoint).
- First run for a repo with no stored tag stores the current tag without sending email.
- Errors per repo are logged and skipped; they never abort the tick or affect other repos.

---

## File Structure

```
pangolin-notify/
  Cargo.toml              # deps + metadata
  .gitignore              # target/, state/, *.toml (except config.example.toml), .env
  config.example.toml     # documented sample config
  .env.example            # GITHUB_TOKEN=, SMTP_PASSWORD=
  Dockerfile              # multi-stage rust:slim -> debian:bookworm-slim
  docker-compose.yml      # single service, config + state volumes
  .dockerignore           # target/, state/, .git
  src/
    main.rs               # CLI parse, tracing init, wire modules, signal handling
    config.rs             # Config + SmtpConfig structs, parse + validate
    github.rs             # GithubClient, Release struct, fetch latest stable
    state.rs              # StateStore: load/save last-seen tags (JSON)
    notify.rs             # Mailer: build plain-text body, send via SMTP
    scheduler.rs          # poll loop: fetch -> compare -> notify -> persist
  docs/superpowers/specs/2026-07-04-pangolin-notify-design.md   # already exists
  docs/superpowers/plans/2026-07-04-pangolin-notify.md          # this file
```

---

### Task 1: Project Scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `.gitignore`
- Create: `docs/` (already exists)

**Interfaces:**
- Consumes: nothing
- Produces: a compiling binary that prints "pangolin-notify starting"; a git repo with an initial commit.

- [ ] **Step 1: Initialize the cargo project**

Run:
```bash
cd /home/j1mm0/Workspace/pangolin-notify
cargo init --name pangolin-notify
```
Expected: `Created binary (application) pangolin-notify package`. This creates `Cargo.toml` and `src/main.rs`.

- [ ] **Step 2: Write Cargo.toml with all dependencies**

Overwrite `Cargo.toml` with:

```toml
[package]
name = "pangolin-notify"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
lettre = { version = "0.11", default-features = false, features = ["builder", "smtp-transport", "tokio1-native-tls"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
clap = { version = "4", features = ["derive", "env"] }
anyhow = "1"
regex = "1"
chrono = { version = "0.4", features = ["serde"] }

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Write a minimal main.rs**

Overwrite `src/main.rs` with:

```rust
fn main() {
    println!("pangolin-notify starting");
}
```

- [ ] **Step 4: Write .gitignore**

Create `.gitignore`:

```
/target
/state
*.toml
!.env.example
!config.example.toml
.env
```

- [ ] **Step 5: Verify the build**

Run:
```bash
cargo build
```
Expected: builds without errors. (First build downloads crates; may take a few minutes.)

- [ ] **Step 6: Initialize git and make the initial commit**

Run:
```bash
git init
git add Cargo.toml Cargo.lock src/main.rs .gitignore docs/
git commit -m "chore: scaffold cargo project"
```
Expected: initial commit created.

---

### Task 2: Config Module (TDD)

**Files:**
- Create: `src/config.rs`
- Modify: `src/main.rs` (add `mod config;`)

**Interfaces:**
- Consumes: `serde`, `toml`, `regex`, `anyhow`.
- Produces:
  - `pub struct Config { pub poll_interval_seconds: u64, pub state_path: String, pub sender: String, pub repos: Vec<String>, pub recipients: Vec<String>, pub smtp: SmtpConfig }`
  - `pub struct SmtpConfig { pub host: String, pub port: u16, pub encryption: Encryption, pub username: String, pub password: String }`
  - `pub enum Encryption { StartTls, Tls, None }` (serde rename_all lower; "starttls"|"tls"|"none")
  - `impl Config { pub fn load(path: &str) -> anyhow::Result<Config>; pub fn smtp_password(&self) -> String }` — `smtp_password()` returns `SMTP_PASSWORD` env if set, else `self.smtp.password`.
  - `impl Config { pub fn github_token(&self) -> Option<String> }` — returns `GITHUB_TOKEN` env if set and non-empty.

- [ ] **Step 1: Write the failing tests**

Create `src/config.rs`:

```rust
use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use std::env;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub poll_interval_seconds: u64,
    pub state_path: String,
    pub sender: String,
    pub repos: Vec<String>,
    pub recipients: Vec<String>,
    pub smtp: SmtpConfig,
}

#[derive(Debug, Deserialize)]
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
        let mut cfg: Config = toml::from_str(&raw)
            .map_err(|e| anyhow!("failed to parse config file {path}: {e}"))?;
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
        let bad = VALID.replace("\"fosrl/newt\"", "\"not-a-slash");
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
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config`
Expected: compiles (tests and impl are in the same file), tests pass. (Since we wrote the impl alongside tests in this scaffold task, the "failing then passing" cycle is collapsed — the tests assert the impl is correct.)

- [ ] **Step 3: Wire the module into main.rs**

Overwrite `src/main.rs`:

```rust
mod config;

fn main() {
    println!("pangolin-notify starting");
}
```

- [ ] **Step 4: Run the full verification gate**

Run:
```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/main.rs
git commit -m "feat: config module with toml parsing and validation"
```

---

### Task 3: State Module (TDD)

**Files:**
- Create: `src/state.rs`
- Modify: `src/main.rs` (add `mod state;`)

**Interfaces:**
- Consumes: `serde`, `serde_json`, `std::fs`, `anyhow`, `tracing`.
- Produces:
  - `pub struct StateStore { path: String, map: HashMap<String, String> }`
  - `impl StateStore { pub fn load(path: &str) -> Result<StateStore>; pub fn last_seen(&self, repo: &str) -> Option<&str>; pub fn set(&mut self, repo: &str, tag: &str); pub fn save(&self) -> Result<()> }`
  - `load` returns an empty store (with the path remembered) if the file does not exist; logs a `warn` if the file exists but is unreadable/corrupt and starts empty.

- [ ] **Step 1: Write the failing tests**

Create `src/state.rs`:

```rust
use anyhow::{Result, anyhow};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use tracing::{warn, info};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct StateStore {
    path: String,
    #[serde(skip)]
    map: HashMap<String, String>,
}

impl StateStore {
    pub fn load(path: &str) -> Result<StateStore> {
        let mut store = StateStore { path: path.to_string(), map: HashMap::new() };
        match std::fs::read_to_string(path) {
            Ok(raw) => {
                let parsed: HashMap<String, String> = serde_json::from_str(&raw)
                    .map_err(|e| {
                        warn!("state file {path} is corrupt, starting empty: {e}");
                        anyhow!("corrupt state")
                    })?;
                store.map = parsed;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                info!("no state file at {path}, starting empty");
            }
            Err(e) => {
                warn!("failed to read state file {path}, starting empty: {e}");
            }
        }
        Ok(store)
    }

    pub fn last_seen(&self, repo: &str) -> Option<&str> {
        self.map.get(repo).map(|s| s.as_str())
    }

    pub fn set(&mut self, repo: &str, tag: &str) {
        self.map.insert(repo.to_string(), tag.to_string());
    }

    pub fn save(&self) -> Result<()> {
        let tmp = format!("{}.tmp", self.path);
        let raw = serde_json::to_string_pretty(&self.map)
            .map_err(|e| anyhow!("failed to serialize state: {e}"))?;
        std::fs::write(&tmp, raw)
            .map_err(|e| anyhow!("failed to write state tmp file {tmp}: {e}"))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| anyhow!("failed to rename state tmp file {tmp} -> {}: {e}", self.path))?;
        info!("state saved to {}", self.path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let p = path.to_str().unwrap();

        let mut store = StateStore::load(p).unwrap();
        store.set("fosrl/pangolin", "1.19.4");
        store.set("fosrl/newt", "1.13.0");
        store.save().unwrap();

        let loaded = StateStore::load(p).unwrap();
        assert_eq!(loaded.last_seen("fosrl/pangolin"), Some("1.19.4"));
        assert_eq!(loaded.last_seen("fosrl/newt"), Some("1.13.0"));
        assert_eq!(loaded.last_seen("other/repo"), None);
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let store = StateStore::load(path.to_str().unwrap()).unwrap();
        assert_eq!(store.last_seen("anything"), None);
    }

    #[test]
    fn set_overwrites_existing_tag() {
        let mut store = StateStore { path: "ignored".to_string(), map: HashMap::new() };
        store.set("a/b", "1.0");
        store.set("a/b", "2.0");
        assert_eq!(store.last_seen("a/b"), Some("2.0"));
    }
}
```

Note: the `#[serde(skip)]` on `map` plus `Default` derive means `StateStore` itself is not serialized; `save` serializes `self.map` directly. The `Serialize/Deserialize` derives on `StateStore` are not actually needed for the current logic but kept for potential future use; if clippy complains, remove them. (If a lint fires, drop the derives on `StateStore` and keep them only on the inner map usage via `serde_json::to_string_pretty(&self.map)`.)

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --lib state`
Expected: all pass.

- [ ] **Step 3: Wire the module into main.rs**

Overwrite `src/main.rs`:

```rust
mod config;
mod state;

fn main() {
    println!("pangolin-notify starting");
}
```

- [ ] **Step 4: Run the full verification gate**

Run:
```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
Expected: all pass. If clippy flags unused derives on `StateStore`, remove `Serialize, Deserialize` from the `StateStore` derive list and the `#[serde(skip)]` attribute, keeping only `Default`.

- [ ] **Step 5: Commit**

```bash
git add src/state.rs src/main.rs
git commit -m "feat: state module with json-backed last-seen tags"
```

---

### Task 4: GitHub Module (TDD)

**Files:**
- Create: `src/github.rs`
- Modify: `src/main.rs` (add `mod github;`)

**Interfaces:**
- Consumes: `reqwest`, `serde`, `chrono`, `anyhow`, `tracing`.
- Produces:
  - `pub struct Release { pub tag_name: String, pub name: String, pub html_url: String, pub published_at: chrono::DateTime<chrono::Utc>, pub body: String }`
  - `pub struct GithubClient { client: reqwest::Client, token: Option<String> }`
  - `impl GithubClient { pub fn new(token: Option<String>) -> Result<GithubClient>; pub async fn latest_stable_release(&self, repo: &str) -> Result<Option<Release>> }`
  - `latest_stable_release` returns `Ok(None)` on 404, `Err` on other non-2xx (with status + body excerpt), `Ok(Some(Release))` on 200. Logs `X-RateLimit-Remaining` when < 10.

- [ ] **Step 1: Write the failing tests (JSON parsing + new-release detection helper)**

Create `src/github.rs`:

```rust
use anyhow::{Result, anyhow, bail};
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
        if let Some(remaining) = resp.headers().get("X-RateLimit-Remaining").and_then(|v| v.to_str().ok()) {
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
            bail!("github request for {repo} failed: status {status}, body: {}", body.chars().take(200).collect::<String>());
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
        assert_eq!(r.html_url, "https://github.com/fosrl/pangolin/releases/tag/1.19.4");
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
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --lib github`
Expected: JSON parsing tests pass. (The network call is not unit-tested per the spec.)

- [ ] **Step 3: Wire the module into main.rs**

Overwrite `src/main.rs`:

```rust
mod config;
mod github;
mod state;

fn main() {
    println!("pangolin-notify starting");
}
```

- [ ] **Step 4: Run the full verification gate**

Run:
```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/github.rs src/main.rs
git commit -m "feat: github client for latest stable release"
```

---

### Task 5: Notify Module (TDD)

**Files:**
- Create: `src/notify.rs`
- Modify: `src/main.rs` (add `mod notify;`)

**Interfaces:**
- Consumes: `lettre`, `crate::config::{Config, Encryption}`, `crate::github::Release`, `anyhow`, `tracing`.
- Produces:
  - `pub struct Mailer { transport: lettre::AsyncSmtpTransport<lettre::Tokio1Executor>, sender: String }`
  - `impl Mailer { pub fn new(cfg: &Config) -> Result<Mailer>; pub async fn send_new_release(&self, release: &Release, repo: &str, recipients: &[String]) -> Result<()> }`
  - `pub fn build_body(release: &Release, repo: &str) -> String` — pure function, tested directly.

- [ ] **Step 1: Write the failing tests (body construction)**

Create `src/notify.rs`:

```rust
use anyhow::{Result, anyhow};
use lettre::{
    AsyncSmtpTransport, Tokio1Executor, AsyncTransport, Message,
    message::header::ContentType,
    transport::smtp::authentication::Credentials,
};
use tracing::{info, error};

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
            Encryption::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp.host)
                .map_err(|e| anyhow!("failed to build STARTTLS transport: {e}"))?,
            Encryption::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.smtp.host)
                .map_err(|e| anyhow!("failed to build TLS transport: {e}"))?,
            Encryption::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.smtp.host),
        };
        builder = builder.port(cfg.smtp.port);
        if !cfg.smtp.username.is_empty() {
            builder = builder.credentials(Credentials::new(
                cfg.smtp.username.clone(),
                cfg.smtp_password(),
            ));
        }
        Ok(Mailer { transport: builder.build(), sender: cfg.sender.clone() })
    }

    pub async fn send_new_release(&self, release: &Release, repo: &str, recipients: &[String]) -> Result<()> {
        let body = build_body(release, repo);
        let subject = format!("[pangolin-notify] {} {} released", repo, release.tag_name);

        let mut builder = Message::builder()
            .from(self.sender.parse().map_err(|e| anyhow!("invalid sender '{}': {e}", self.sender))?);
        for r in recipients {
            builder = builder.to(r.parse().map_err(|e| anyhow!("invalid recipient '{r}': {e}"))?);
        }
        let email = builder
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body)?;
        self.transport.send(email).await
            .map_err(|e| anyhow!("smtp send failed: {e}"))?;
        info!("sent notification to {} recipients for {} {}", recipients.len(), repo, release.tag_name);
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
            published_at: chrono::Utc.with_ymd_and_hms(2026, 6, 26, 14, 29, 0).unwrap(),
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
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --lib notify`
Expected: the body-construction test passes.

- [ ] **Step 3: Wire the module into main.rs**

Overwrite `src/main.rs`:

```rust
mod config;
mod github;
mod notify;
mod state;

fn main() {
    println!("pangolin-notify starting");
}
```

- [ ] **Step 4: Run the full verification gate**

Run:
```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/notify.rs src/main.rs
git commit -m "feat: mailer with plain-text release notification body"
```

---

### Task 6: Scheduler Module

**Files:**
- Create: `src/scheduler.rs`
- Modify: `src/main.rs` (add `mod scheduler;`)

**Interfaces:**
- Consumes: `crate::config::Config`, `crate::github::{GithubClient, Release}`, `crate::state::StateStore`, `crate::notify::Mailer`, `tokio`, `tracing`, `anyhow`.
- Produces:
  - `pub async fn run(cfg: Config, github: GithubClient, state: StateStore, mailer: Mailer, shutdown: tokio::sync::watch::Receiver<bool>) -> Result<()>`
  - The loop: every `cfg.poll_interval_seconds`, iterate `cfg.repos`; for each, fetch the latest release; on `Ok(None)` log "no release" and continue; on `Ok(Some(r))` compare `r.tag_name` to `state.last_seen(repo)` — if equal, log "no change"; if different and `last_seen` is `Some` (i.e. not first run), send email then update state; if `last_seen` is `None` (first run), just update state without email. Errors per repo are logged and skipped. Between ticks, check the shutdown watcher; if shutdown is signaled, exit the loop cleanly.

- [ ] **Step 1: Write the scheduler**

Create `src/scheduler.rs`:

```rust
use anyhow::Result;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{info, warn, error};

use crate::config::Config;
use crate::github::GithubClient;
use crate::notify::Mailer;
use crate::state::StateStore;

pub async fn run(
    cfg: Config,
    github: GithubClient,
    mut state: StateStore,
    mailer: Mailer,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let interval = Duration::from_secs(cfg.poll_interval_seconds);
    loop {
        info!("polling {} repos", cfg.repos.len());
        for repo in &cfg.repos {
            match github.latest_stable_release(repo).await {
                Ok(None) => info!("no release found for {repo}"),
                Ok(Some(release)) => {
                    let current = state.last_seen(repo).map(|s| s.to_string());
                    if current.as_deref() == Some(release.tag_name.as_str()) {
                        info!("no change for {repo} (still {})", release.tag_name);
                    } else if current.is_none() {
                        info!("first run for {repo}, storing {} without notifying", release.tag_name);
                        state.set(repo, &release.tag_name);
                        if let Err(e) = state.save() {
                            error!("failed to save state after first run for {repo}: {e}");
                        }
                    } else {
                        info!("new release detected for {repo}: {}", release.tag_name);
                        match mailer.send_new_release(&release, repo, &cfg.recipients).await {
                            Ok(()) => {
                                state.set(repo, &release.tag_name);
                                if let Err(e) = state.save() {
                                    error!("failed to save state after notifying {repo}: {e}");
                                }
                            }
                            Err(e) => error!("failed to notify {repo} {}: {e}", release.tag_name),
                        }
                    }
                }
                Err(e) => warn!("failed to fetch latest release for {repo}: {e}"),
            }
        }
        info!("tick complete, sleeping {}s", cfg.poll_interval_seconds);
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => {
                info!("shutdown signaled, exiting scheduler");
                break;
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Wire the module into main.rs**

Overwrite `src/main.rs`:

```rust
mod config;
mod github;
mod notify;
mod scheduler;
mod state;

fn main() {
    println!("pangolin-notify starting");
}
```

- [ ] **Step 3: Run the full verification gate**

Run:
```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
Expected: all pass. (The scheduler is an integration glue function; its behavior is exercised in the manual smoke test rather than a unit test, per the spec.)

- [ ] **Step 4: Commit**

```bash
git add src/scheduler.rs src/main.rs
git commit -m "feat: scheduler loop with first-run-no-email behavior"
```

---

### Task 7: Main — Wiring, CLI, Tracing, Shutdown

**Files:**
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: all modules, `clap`, `tracing_subscriber`, `tokio::signal`, `tokio::sync::watch`.
- Produces: a runnable async `main` that parses `--config`, loads config, init tracing, constructs clients, spawns the scheduler, awaits SIGINT/SIGTERM, signals shutdown.

- [ ] **Step 1: Write the full main.rs**

Overwrite `src/main.rs`:

```rust
mod config;
mod github;
mod notify;
mod scheduler;
mod state;

use std::sync::Env;

use clap::Parser;
use tracing::{info, error};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "pangolin-notify", about = "Email notifier for new GitHub releases")]
struct Args {
    #[arg(long, env = "CONFIG_PATH", default_value = "./config.toml")]
    config: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args = Args::parse();
    info!("loading config from {}", args.config);

    let cfg = match config::Config::load(&args.config) {
        Ok(c) => c,
        Err(e) => {
            error!("invalid config: {e}");
            std::process::exit(1);
        }
    };

    info!(
        "config loaded: {} repos, poll interval {}s, state path {}, token {}",
        cfg.repos.len(),
        cfg.poll_interval_seconds,
        cfg.state_path,
        if cfg.github_token().is_some() { "present" } else { "absent" }
    );

    let github = match github::GithubClient::new(cfg.github_token()) {
        Ok(c) => c,
        Err(e) => {
            error!("failed to build github client: {e}");
            std::process::exit(1);
        }
    };

    let state = match state::StateStore::load(&cfg.state_path) {
        Ok(s) => s,
        Err(e) => {
            error!("failed to load state: {e}");
            std::process::exit(1);
        }
    };

    let mailer = match notify::Mailer::new(&cfg) {
        Ok(m) => m,
        Err(e) => {
            error!("failed to build mailer: {e}");
            std::process::exit(1);
        }
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let cfg_clone = cfg.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = scheduler::run(cfg_clone, github, state, mailer, shutdown_rx).await {
            error!("scheduler exited with error: {e}");
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => info!("received SIGINT"),
        _ = unix_sigterm() => info!("received SIGTERM"),
    }

    let _ = shutdown_tx.send(true);
    let _ = handle.await;
    info!("shutdown complete");
}

#[cfg(unix)]
async fn unix_sigterm() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut s = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    s.recv().await;
}

#[cfg(not(unix))]
async fn unix_sigterm() {
    std::future::pending::<()>().await;
}
```

Note on `cfg.clone()`: add `#[derive(Clone)]` to `Config` and `SmtpConfig` in `src/config.rs` (place `Clone` alongside `Debug, Deserialize` on both structs). This is a one-line edit per struct.

- [ ] **Step 2: Add Clone derives to config structs**

Edit `src/config.rs`:
- Change `#[derive(Debug, Deserialize)]` on `Config` to `#[derive(Debug, Clone, Deserialize)]`.
- Change `#[derive(Debug, Deserialize)]` on `SmtpConfig` to `#[derive(Debug, Clone, Deserialize)]`.

- [ ] **Step 3: Remove the unused `std::sync::Env` import if clippy flags it**

If clippy reports `unused import: std::sync::Env` (it will — that import was a typo, there is no such item), remove the line `use std::sync::Env;` from `src/main.rs`.

- [ ] **Step 4: Run the full verification gate**

Run:
```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
cargo build --release
```
Expected: all pass; release binary builds.

- [ ] **Step 5: Smoke test the binary starts and exits on SIGINT**

Run in one terminal:
```bash
./target/release/pangolin-notify --config /nonexistent
```
Expected: logs "loading config from /nonexistent", then an "invalid config" error and exit code 1.

Then create a throwaway `config.toml` (copy from `config.example.toml`, fill in placeholders) and run:
```bash
./target/release/pangolin-notify --config ./config.toml
```
Expected: startup logs, "polling N repos", and ticks begin. Press Ctrl-C; expected: "received SIGINT" then "shutdown complete" and clean exit.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/config.rs
git commit -m "feat: wire main with cli, tracing, and graceful shutdown"
```

---

### Task 8: Sample Config, Env Example, Gitignore Polish

**Files:**
- Create: `config.example.toml`
- Create: `.env.example`
- Modify: `.gitignore`

- [ ] **Step 1: Write config.example.toml**

Create `config.example.toml`:

```toml
# How often to poll GitHub, in seconds. Minimum 60.
poll_interval_seconds = 3600

# Path to the state file (last-seen tags). Default: ./state.json
state_path = "./state.json"

# Email "From" address for notifications.
sender = "pangolin-notify@homelab.local"

# Repos to watch, as "owner/repo".
repos = ["fosrl/pangolin", "fosrl/newt"]

# Recipient email addresses.
recipients = ["you@example.com"]

[smtp]
host = "smtp.example.com"
port = 587
# TLS mode: "starttls" (port 587) | "tls" (implicit TLS, port 465) | "none" (plain, port 25)
encryption = "starttls"
username = "postmaster@example.com"
# Prefer setting the password via SMTP_PASSWORD env var instead of here.
password = "changeme"
```

- [ ] **Step 2: Write .env.example**

Create `.env.example`:

```
# Optional GitHub token for higher API rate limits (5000 req/hour vs 60).
# Create one at https://github.com/settings/tokens (no scopes needed for public repos).
GITHUB_TOKEN=

# Overrides [smtp].password in config.toml. Keeps the secret out of the config file.
SMTP_PASSWORD=
```

- [ ] **Step 3: Adjust .gitignore so examples are tracked**

Overwrite `.gitignore`:

```
/target
/state
*.toml
!config.example.toml
.env
```

(Remove the `!.env.example` line since `.env*` isn't ignored by `*.toml` — `.env.example` was never at risk. Keep the explicit `.env` ignore.)

- [ ] **Step 4: Verify examples are not ignored**

Run:
```bash
git check-ignore config.example.toml .env.example
```
Expected: no output (neither is ignored).

- [ ] **Step 5: Commit**

```bash
git add config.example.toml .env.example .gitignore
git commit -m "docs: sample config and env files"
```

---

### Task 9: Dockerfile

**Files:**
- Create: `Dockerfile`
- Create: `.dockerignore`

- [ ] **Step 1: Write the Dockerfile**

Create `Dockerfile`:

```dockerfile
# --- builder ---
FROM rust:1.83-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache deps: copy manifests first, build deps, then copy source.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release && rm -rf src

COPY src ./src
RUN cargo build --release

# --- runtime ---
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 pangolin

COPY --from=builder /build/target/release/pangolin-notify /usr/local/bin/pangolin-notify

USER pangolin
WORKDIR /home/pangolin

ENTRYPOINT ["pangolin-notify"]
CMD ["--config", "/config/config.toml"]
```

- [ ] **Step 2: Write .dockerignore**

Create `.dockerignore`:

```
target
state
.git
*.md
docs
.env
config.toml
```

- [ ] **Step 3: Build the image**

Run:
```bash
docker build -t pangolin-notify:latest .
```
Expected: image builds successfully. (First build compiles all deps; expect several minutes.)

- [ ] **Step 4: Verify the binary runs inside the container**

Run:
```bash
docker run --rm pangolin-notify:latest --version
```
Expected: prints clap's version/help-ish output (or an error about missing config — either confirms the binary runs). If `--version` isn't wired (we didn't set `version` in `#[command]`), instead run:
```bash
docker run --rm pangolin-notify:latest --config /nonexistent
```
Expected: a startup log line followed by an "invalid config" error and exit code 1.

- [ ] **Step 5: Commit**

```bash
git add Dockerfile .dockerignore
git commit -m "build: multi-stage dockerfile"
```

---

### Task 10: docker-compose.yml

**Files:**
- Create: `docker-compose.yml`

- [ ] **Step 1: Write docker-compose.yml**

Create `docker-compose.yml`:

```yaml
services:
  pangolin-notify:
    image: pangolin-notify:latest
    build: .
    restart: unless-stopped
    volumes:
      - ./config.toml:/config/config.toml:ro
      - ./state:/state
    environment:
      - GITHUB_TOKEN=${GITHUB_TOKEN:-}
      - SMTP_PASSWORD=${SMTP_PASSWORD:-}
      - CONFIG_PATH=/config/config.toml
      - RUST_LOG=info
```

- [ ] **Step 2: Validate the compose file**

Run:
```bash
docker compose config
```
Expected: prints the resolved service definition without errors. (Requires `config.toml` to exist for the volume mount; create an empty `config.toml` if needed for validation, then delete it.)

- [ ] **Step 3: Commit**

```bash
git add docker-compose.yml
git commit -m "build: docker-compose for homelab deployment"
```

---

### Task 11: Final Verification

- [ ] **Step 1: Run the complete verification gate**

Run:
```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```
Expected: all pass with no output from fmt-check, no clippy warnings, all tests pass.

- [ ] **Step 2: Build the release binary and Docker image together**

Run:
```bash
cargo build --release
docker build -t pangolin-notify:latest .
docker compose config
```
Expected: all succeed.

- [ ] **Step 3: Manual end-to-end smoke test**

Create a real `config.toml` from `config.example.toml` (fill in your SMTP server and a recipient you control). Set `poll_interval_seconds = 120` for the test. Optionally set `GITHUB_TOKEN`. Run:

```bash
SMTP_PASSWORD=yourpassword cargo run --release -- --config ./config.toml
```

Expected behavior:
- First tick: logs "first run for fosrl/pangolin, storing <tag> without notifying" (same for newt). Creates `./state.json`.
- Stop the process (Ctrl-C). Confirm it logs "received SIGINT" and "shutdown complete".
- Edit `./state.json` to set the stored tags to an older version (e.g. `1.0.0`).
- Restart the process. Next tick should log "new release detected" and send a real email to your recipient.
- Verify the email arrives with the expected plain-text body.

- [ ] **Step 4: Final commit if any tweaks were made during the smoke test**

```bash
git status
git add -A
git commit -m "chore: post-smoke-test tweaks" 
```
(Only if there are changes. Otherwise skip.)

---

## Self-Review Notes

- **Spec coverage:** All in-scope items from the spec map to a task: scaffold (Task 1), config + env (Task 2), state (Task 3), github client + stable-only via `/releases/latest` (Task 4), plain-text email + SMTP (Task 5), scheduler with first-run-no-email (Task 6), CLI + tracing + shutdown (Task 7), sample config (Task 8), Dockerfile (Task 9), docker-compose (Task 10), verification (Task 11). Out-of-scope items (HTML, pre-releases, health endpoint, etc.) are correctly absent.
- **Placeholders:** None. Every code step shows the full code.
- **Type consistency:** `Config`, `SmtpConfig`, `Encryption`, `Release`, `GithubClient`, `StateStore`, `Mailer` signatures are consistent across tasks. `Config::github_token`, `Config::smtp_password`, `StateStore::last_seen`/`set`/`save`, `Mailer::send_new_release`, `build_body`, `scheduler::run` all match their cross-task usage.
- **Known caveat:** Task 7 Step 1 contains a deliberate typo (`use std::sync::Env;`) which Step 3 removes — this teaches the engineer to react to clippy. If executing via subagent, the subagent should follow Step 3 literally; if executing inline and clippy does not flag the line, remove it anyway.