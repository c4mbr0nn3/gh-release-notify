# gh-release-notify — Design Spec

**Date:** 2026-07-04
**Status:** Approved (brainstorming complete)
**Author:** j1mm0 (with opencode)

## 1. Overview

**gh-release-notify** is a small long-running Rust daemon that periodically checks a configurable list of GitHub repositories for new **stable** releases and sends a plain-text email notification to a configurable list of recipients via SMTP when a new release is detected.

The motivating use case is monitoring [fosrl/pangolin](https://github.com/fosrl/pangolin/releases) and [fosrl/newt](https://github.com/fosrl/newt/releases) from a homelab deployment. The project doubles as a deliberate learning vehicle for Rust (the author's first Rust project, coming from a .NET background).

## 2. Scope

### In scope

- Long-running async daemon (Tokio runtime) with a configurable poll interval.
- Configurable repository list (`owner/repo` strings) supplied in a TOML config file.
- Polls the GitHub REST API endpoint `GET /repos/{owner}/{repo}/releases/latest` for each repo. This endpoint returns only the latest **non-prerelease** release, so release candidates (e.g. `1.19.0-rc.1`) are naturally excluded.
- Optional GitHub personal access token read from an environment variable, used to raise the unauthenticated rate limit (60 req/hour/IP) to 5000 req/hour.
- Persists the last-seen release tag per repo to a JSON file on disk so notifications are not re-sent after a restart.
- Sends a plain-text email via SMTP (using the `lettre` crate) to a configurable recipient list.
- Container deployment artifacts: a multi-stage `Dockerfile` and a `docker-compose.yml` for homelab use, with config and state mounted as volumes.

### Out of scope (YAGNI)

- HTML email / multipart bodies.
- Pre-release (rc/beta/alpha) notifications.
- An HTTP health or readiness endpoint.
- Any web UI or admin interface.
- Notification channels other than email (no webhooks, Slack, etc.).
- Custom retry/backoff logic beyond "try again next tick".
- Database-backed state (SQLite, sled, etc.).
- Per-repo override of the stable-only/pre-release filter.
- CI workflow (can be added in a later iteration).

## 3. Architecture

A single binary running one async task. Module layout:

```
src/
  main.rs          # entry: load config, init tracing, spawn scheduler, await shutdown signal
  config.rs        # parse & validate config.toml
  github.rs        # GitHub release client: fetch latest stable release for a repo
  state.rs         # load/save state.json (last-seen tag per repo)
  notify.rs        # build plain-text email body + send via SMTP (lettre)
  scheduler.rs     # poll loop: fetch -> compare to state -> notify on new -> persist state
```

### Component responsibilities

**`config`** — Reads `config.toml` from a path (default `./config.toml`; overridable via `--config` CLI argument and `CONFIG_PATH` environment variable). Validates all fields (see §4). Exits with a clear, field-specific error message on invalid config — no silent defaults for SMTP credentials. Exposes a `Config` struct consumed by the rest of the app.

**`github`** — A `GithubClient` struct holding an optional bearer token and a `reqwest::Client` (30s timeout). Method `latest_stable_release(repo: &str) -> Result<Option<Release>>`. Hits `GET /repos/{repo}/releases/latest`. A 404 is treated as "no release yet" and returns `Ok(None)`. Honors and logs the `X-RateLimit-Remaining` header. Returns a `Release { tag_name, name, html_url, published_at, body }` parsed via serde.

**`state`** — A `StateStore` backed by a JSON file containing a `HashMap<String, String>` mapping `repo` → `last_seen_tag`. Methods: `load(path) -> Result<StateStore>` (returns empty state if the file is missing) and `save(&self) -> Result<()>` (atomic: write to `<path>.tmp` then rename). Logs warnings on read errors but starts fresh.

**`notify`** — A `Mailer` wrapping lettre's `AsyncSmtpTransport`. Method `send_new_release(&self, release: &Release, repo: &str, recipients: &[String]) -> Result<()>`. Constructs a plain-text body containing: repo name, tag, published date, release URL, and the release notes body. Subject line: `[gh-release-notify] {repo} {tag} released`.

**`scheduler`** — `run(config, github, state, mailer, shutdown)` loop. Each tick (every `poll_interval_seconds`): iterate over `config.repos`, fetch the latest release, compare its `tag_name` to the value stored in `state` for that repo, and on a mismatch send an email and update the stored tag. Per-repo errors are logged but do not abort the tick or affect other repos. On the first run for a repo with no stored tag, the current latest tag is stored **without** sending an email (avoids spamming on first deploy). The loop checks a shutdown signal between ticks.

**`main`** — Parses the `--config` CLI argument, loads config, initializes `tracing_subscriber` (fmt, info level), constructs `GithubClient`, `StateStore`, `Mailer`, spawns the scheduler task, and listens for SIGINT/SIGTERM via `tokio::signal` for graceful shutdown.

### Data flow

```
config.toml -> scheduler tick
            -> github: fetch latest stable release for repo
            -> compare tag against state.json entry
            -> on mismatch: notify (SMTP send) then update state.json
            -> on match: no-op
```

### Graceful shutdown

On SIGINT/SIGTERM, the scheduler finishes the current tick if mid-flight, then exits. State is only written **after** a notification has been sent (or skipped), so a crash mid-tick simply causes the next tick to re-check and re-notify if needed.

## 4. Configuration Schema

`config.toml`:

```toml
# How often to poll GitHub, in seconds. Minimum 60.
poll_interval_seconds = 3600

# Path to the state file (last-seen tags). Default: ./state.json
state_path = "./state.json"

# Email "From" address for notifications.
sender = "gh-release-notify@homelab.local"

# Repos to watch, as "owner/repo".
repos = ["fosrl/pangolin", "fosrl/newt"]

# Recipient email addresses.
recipients = ["you@example.com", "ops@example.com"]

[smtp]
host = "smtp.example.com"
port = 587
# TLS mode: "starttls" (port 587) | "tls" (implicit TLS, port 465) | "none" (plain, port 25, not recommended)
encryption = "starttls"
username = "postmaster@example.com"
password = "smtp-password-here"
```

### Environment variables

| Variable        | Purpose                                                                  | Default        |
|-----------------|--------------------------------------------------------------------------|----------------|
| `CONFIG_PATH`   | Path to the config file.                                                 | `./config.toml`|
| `GITHUB_TOKEN`  | Optional GitHub PAT. If set, sent as `Authorization: Bearer <token>`.   | (unset)        |
| `SMTP_PASSWORD` | If set, overrides `[smtp].password`. Lets you keep the secret out of the config file. | (unset; falls back to `[smtp].password`) |
| `RUST_LOG`      | tracing filter directive (e.g. `info`, `debug`, `gh_release_notify=debug`).| `info`         |

### Validation rules (fail fast at startup)

- `poll_interval_seconds >= 60`.
- `repos` is non-empty; each entry matches `^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$`.
- `recipients` is non-empty; each entry is a syntactically valid email (regex check, no MX lookup).
- `sender` is a syntactically valid email.
- `[smtp].host` is non-empty; `port` is in `1..=65535`.
- `[smtp].encryption` is one of `starttls | tls | none`.
- If `[smtp].username` is set, then either `[smtp].password` or `SMTP_PASSWORD` must be present.

### Precedence

Environment variables override config file values where they overlap: `CONFIG_PATH`, `GITHUB_TOKEN`, `SMTP_PASSWORD`.

## 5. Error Handling & Logging

### Logging

`tracing` + `tracing_subscriber` (fmt layer, info level by default; controlled by `RUST_LOG`). One structured log line per significant event:

- **Startup:** config path, repo count, poll interval, token present (yes/no), state path.
- **Tick start:** "polling N repos".
- **Per repo:** "fetching latest for {repo}", "no release found" (404), "no change (still {tag})", "new release detected: {tag}".
- **Rate limit:** log `X-RateLimit-Remaining` when it drops below 10.
- **Email:** "sent notification to {n} recipients for {repo} {tag}", or error with lettre detail.
- **State:** warnings on read errors, info on writes.
- **Shutdown:** "received SIGTERM, finishing current tick", "shutdown complete".

Errors never panic. All fallible operations return `Result` and are handled with `?` at task boundaries or logged-and-continued.

### Error handling strategy

| Failure mode                    | Action                                                                                                  |
|---------------------------------|---------------------------------------------------------------------------------------------------------|
| Config validation error         | Fatal at startup; print field-specific message; exit code 1.                                            |
| GitHub fetch error (per repo)   | Log `warn`; skip that repo this tick; do not update its state (retries next tick).                       |
| GitHub 403 (rate-limited)       | Log `error`; skip all repos this tick; do not update any state. Resume next tick.                       |
| State load error                | Log `warn`; start with empty state (next tick behaves as first-run for every repo).                     |
| State save error                | Log `error`; notification already sent, so next tick may re-send. Acceptable and documented.            |
| SMTP send error                 | Log `error`; do **not** update state for that repo (retry the email next tick). May send multiple emails once recovered — acceptable and documented. |
| HTTP network/timeout            | reqwest 30s timeout; log `warn`; skip repo; no state update.                                            |

No retry/backoff logic beyond "try again next tick". The poll interval is the natural retry cadence — YAGNI for a homelab notifier.

## 6. Deployment

### Dockerfile

Multi-stage build:
- **Builder stage:** `rust:slim` base, `cargo build --release`.
- **Runtime stage:** `debian:bookworm-slim` (needed for lettre's native TLS dependencies; distroless would require extra care). Copies the release binary. Runs as a non-root user. `ENTRYPOINT` runs the binary with `--config /config/config.toml`.

### docker-compose.yml

```yaml
services:
  gh-release-notify:
    image: gh-release-notify:latest
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

`state_path` in the config file points to `/state/state.json`. A `.env` file alongside `docker-compose.yml` provides `GITHUB_TOKEN` and `SMTP_PASSWORD`.

### Supporting files

- `config.example.toml` — checked in, with comments explaining each field. User copies to `config.toml` and edits.
- `.env.example` — `GITHUB_TOKEN=` and `SMTP_PASSWORD=` placeholders.
- `.dockerignore` — excludes `target/`, `state/`, `.git`.

## 7. Testing

Kept deliberately thin to match the "very thin service" intent, but covers the logic worth pinning.

### Unit tests (in-module `#[cfg(test)]`)

- **`config.rs`:** parse a sample TOML, assert all fields populate correctly; assert validation rejects each bad case (missing repos, empty repos, bad email, port out of range, bad encryption value, username without password).
- **`state.rs`:** save → load round-trip preserves the map; loading a missing file returns an empty state.
- **`github.rs`:** parse a sample GitHub API JSON response into a `Release`; tag-comparison helper that decides "new release detected".
- **`notify.rs`:** email body construction (string content assertions) — does not perform a real SMTP send.

### Not tested

- No integration tests against the real GitHub API or a real SMTP server. Both are thin wrappers over well-tested crates (`reqwest`, `lettre`); testing them would be flaky and low-value.

### Manual smoke test

Run locally with a short `poll_interval_seconds = 60` and a fake/single recipient to confirm a tick completes. Verify the first-run-no-email behavior with an empty/missing state file.

### Verification gate

- `cargo test`
- `cargo clippy -- -D warnings`
- `cargo fmt --check`

These three commands must pass before any claim of "done".

## 8. Rust Crates

| Crate                  | Purpose                                      |
|------------------------|----------------------------------------------|
| `tokio`                | Async runtime, signals, task spawning.      |
| `reqwest`              | HTTP client for the GitHub API.              |
| `serde` + `serde_json` | Deserialization (config, GitHub JSON, state).|
| `toml`                 | Config file parsing.                         |
| `lettre`               | SMTP transport (async, with Tokio feature).  |
| `tracing`              | Structured logging facade.                   |
| `tracing-subscriber`   | Logging implementation (fmt layer).          |
| `clap`                 | CLI argument parsing (`--config`).           |
| `anyhow`               | Error handling ergonomics at the binary edge.|
| `regex`                | Email and `owner/repo` format validation.    |
| `chrono`               | Parsing and formatting `published_at`.       |

Dev-dependencies: `tempfile` (for state round-trip tests in a temp dir).

## 9. Decisions Log

| Decision                       | Choice                                  | Why                                                                                          |
|--------------------------------|-----------------------------------------|----------------------------------------------------------------------------------------------|
| Execution model                | Long-running daemon                     | Simplest to operate as one binary/container; good async Rust learning vehicle.               |
| State persistence              | JSON file on disk                       | Trivial to inspect/reset; only two key-value pairs; good serde/file I/O learning.            |
| Pre-release handling           | Stable only                             | Matches user need; `/releases/latest` endpoint naturally excludes pre-releases.              |
| Config format                  | TOML                                    | Human-friendly, comments allowed, idiomatic Rust config format.                              |
| Email format                   | Plain text only                         | Reliable rendering; minimal code; focus on string handling.                                  |
| Deployment artifacts           | Dockerfile + docker-compose             | Targeted at homelab deployment.                                                              |
| Repo list                      | Configurable in TOML                    | Slightly more flexible than hardcoding; tiny extra config surface.                           |
| GitHub auth                    | Optional token from env                 | Default unauthenticated is fine for 2 repos/hour; token path available for shared IPs.       |
| Rust async stack               | Tokio + reqwest + lettre                | Dominant modern Rust service stack; best learning vehicle.                                   |