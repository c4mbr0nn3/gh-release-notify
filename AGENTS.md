# AGENTS.md

## Project

pangolin-notify: a long-running Rust async daemon that polls a configurable list of GitHub repos for new stable releases and sends plain-text email notifications via SMTP. Deploys as a Docker/podman container for homelab use.

## Stack

- Rust edition 2021.
- Async runtime: `tokio` (full features).
- HTTP: `reqwest` (json feature).
- Config: `toml` + `serde`. State: `serde_json` on disk.
- Email: `lettre` 0.11 (`builder`, `smtp-transport`, `tokio1-native-tls` features).
- Logging: `tracing` + `tracing-subscriber` (env-filter).
- CLI: `clap` (derive + env features).
- Errors: `anyhow` at binary edges; fallible ops return `Result`.
- Validation: `regex`. Dates: `chrono` (serde feature).

## Layout

```
src/
  main.rs        # entry: CLI, tracing, wire modules, signal handling
  config.rs      # Config + SmtpConfig + Encryption: parse & validate config.toml
  github.rs      # GithubClient + Release + GithubError: fetch latest stable release
  state.rs       # StateStore: JSON-backed last-seen tags (atomic save)
  notify.rs      # Mailer + build_body: plain-text email via SMTP (lettre async)
  scheduler.rs   # run(): poll loop, first-run-no-email, graceful shutdown
docs/superpowers/specs/    # design spec
docs/superpowers/plans/    # implementation plan
.superpowers/sdd/          # SDD scratch (git-ignored): briefs, reports, ledger
```

## Verification gate

Run before claiming any task is done, before any commit:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

For release/deploy checks also run `cargo build --release`. Container image build uses `podman build` (no `docker` on this system); `docker-compose.yml` is provided but not testable here (no docker compose).

## Rules

- **No comments in Rust source files** unless explicitly requested. The `config.example.toml`, `.env.example`, `Dockerfile`, and `docker-compose.yml` are documentation files and MAY contain comments.
- **No `unwrap`/`expect`/`panic` in non-test code.** The `Regex::new(...).unwrap()` calls in `config.rs` and the `expect("install SIGTERM handler")` in `main.rs::unix_sigterm` are deliberate, plan-mandated exceptions (compile-time-constant patterns / fatal-startup path).
- **All fallible operations return `Result`.** Handle with `?` at task boundaries or log-and-continue; never swallow errors silently.
- **Out of scope (do NOT add):** HTML email, pre-release notifications, HTTP health endpoint, web UI, retry/backoff beyond "try again next tick", DB-backed state, per-repo stable/pre-release override.
- **State semantics:** first run for a repo with no stored tag stores the tag WITHOUT sending email. On SMTP failure, do NOT update state (retry email next tick). State save is atomic (write `<path>.tmp` then rename).

## Config & env

- Config file: TOML, path via `--config` CLI arg or `CONFIG_PATH` env, default `./config.toml`.
- Env overrides: `SMTP_PASSWORD` overrides `[smtp].password`; `GITHUB_TOKEN` (if set and non-empty) used for bearer auth; `RUST_LOG` controls tracing filter (default `info`).
- For Docker/compose deployment `state_path` must be `/state/state.json` (the mounted volume), not `./state.json`.

## Testing approach

- Unit tests live in-module under `#[cfg(test)]`. Dev-dep: `tempfile` for filesystem tests.
- No integration tests against real GitHub or real SMTP — both are thin wrappers over well-tested crates.
- The scheduler is glue; its behavior is exercised manually (smoke test with short `poll_interval_seconds`), not unit-tested.
- Run a focused test while iterating: `cargo test config` / `cargo test state` / etc. Run the full suite once before committing.

## SDD artifacts

- Spec: `docs/superpowers/specs/2026-07-04-pangolin-notify-design.md`
- Plan: `docs/superpowers/plans/2026-07-04-pangolin-notify.md`
- Progress ledger: `.superpowers/sdd/progress.md` (git-ignored scratch; recover from `git log` if destroyed).
- Per-task briefs and reports live under `.superpowers/sdd/`.