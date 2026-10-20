# AGENTS.md

## Project

gh-release-notify: a long-running Rust async daemon that polls a configurable
list of GitHub repos for new stable releases and sends plain-text email
notifications via SMTP. An embedded web UI shows status and edits the config.
Deploys as a Docker/podman container for homelab use.

## Stack

- Rust 2021, tokio async runtime. The dependency set and feature flags live in
  `Cargo.toml`; read it rather than trusting a copy here.
- Errors: `anyhow` for propagation, `GithubError` for typed GitHub failures.
- Web UI: axum 0.8, vanilla JS, one hand-written stylesheet (`src/ui/ui.css`),
  embedded with `include_str!`. No npm, CSS framework, or JS build step. All UI
  work follows `docs/UI-GUIDELINES.md`.

## Layout

Module map: `README.md` § Project layout. UI assets live in `src/ui/`.

## Commit conventions

- Follow [Conventional Commits](https://www.conventionalcommits.org/): `<type>: <subject>`.
- Types: `feat`, `fix`, `docs`, `chore`, `build`, `ci`, `refactor`, `test`.
- Subject line short and concise, **<= 72 chars**, imperative mood, no trailing period.
- **No body allowed.** Subject only.

## Verification gate

Run before claiming any task is done, before any commit:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

- Run the focused test while iterating (`cargo test config`, ...); run the full
  suite once before committing.
- For release/deploy checks also run `cargo build --release`; for container
  changes run `./scripts/test-container.sh`.
- `scripts/release.sh` and `.github/workflows/release.yml` repeat the gate
  (`cargo fmt --check` in both) before releasing or building.
- Container commands: use `docker` when available, else `podman`. Exception:
  `scripts/test-container.sh` requires `podman` and exits if it is missing. For
  compose, probe `docker compose`, then `podman compose` or `docker-compose`,
  and use whichever is present. If no container tool is available, report
  container builds as not testable here.

## Rules

- **No comments in Rust source files** unless explicitly requested. The
  `config.example.toml`, `.env.example`, `Dockerfile`, and `docker-compose.yml`
  are documentation files and MAY contain comments.
- **No `unwrap`/`expect`/`panic` in non-test code.** The `Regex::new(...).unwrap()`
  calls in `config.rs` and the `expect("install SIGTERM handler")` in
  `main.rs::unix_sigterm` are deliberate, plan-mandated exceptions
  (compile-time-constant patterns / fatal-startup path).
- **All fallible operations return `Result`.** Handle with `?` at task
  boundaries or log-and-continue; never swallow errors silently.
- **Out of scope (do NOT add):** HTML email, pre-release notifications,
  retry/backoff beyond "try again next tick", DB-backed state, per-repo
  stable/pre-release override.
- **State semantics:** first run for a repo with no stored tag stores the tag
  WITHOUT sending email. On SMTP failure, do NOT update state (retry email next
  tick). State save is atomic (write `<path>.tmp` then rename).

## Config & env

- Config file: TOML, path via `--config` CLI arg or `CONFIG_PATH` env, default
  `./config.toml`.
- Env overrides apply only when non-empty; an empty value falls through to the
  file: `SMTP_PASSWORD` over `[smtp].password`, `ADMIN_TOKEN` over
  `[ui].admin_token`, `GITHUB_TOKEN` enables bearer auth. `RUST_LOG` controls
  the tracing filter (default `info`).
- Optional `sender_name` sets the From display name.
- State file path: `state_path` in the config, overridable at runtime via
  `--state-path` CLI arg or `STATE_PATH` env (empty override is a startup
  error). The override is not written back to the config file.
- For Docker/compose deployment `state_path` must be `/state/state.json` (the
  mounted volume), not `./state.json`.
- Optional `cron_expression` in config: standard 5-field cron expression (UTC)
  that takes precedence over `poll_interval_seconds` when present.
  Auto-prepends a seconds field for the `cron` crate. Day-of-week:
  1=Sunday .. 7=Saturday.
- `[ui]` section (optional): `bind_addr` (default `127.0.0.1`), `port`
  (default `8080`), `admin_token` (default empty = no login),
  `trust_proxy_auth` (default false).
- `UI_BIND_ADDR` and `UI_PORT` override `[ui].bind_addr`/`[ui].port` at
  startup and are never written back; the compose file sets
  `UI_BIND_ADDR=0.0.0.0`.

## Web UI

An axum 0.8 server embedded in the daemon: a single `src/ui/index.html` with
vanilla JS (`src/ui/app.js`) and one hand-written stylesheet (`src/ui/ui.css`),
all pulled in with `include_str!` via `src/ui/assets.rs`. No JS build step, no
npm, and no CSS framework.

**Design contract:** read `docs/UI-GUIDELINES.md` before changing anything under
`src/ui/`; every UI change must follow it. If a rule must change, update the
guideline in the same change.

Invariants:

- Exactly one auth mode at startup: **token** (session cookie + optional
  `Authorization: Bearer`), **proxy** (`Remote-User` header, spoofable if the
  port is directly reachable), or **open**. `admin_token` and
  `trust_proxy_auth = true` together is a **startup error**.
- Endpoints report secret *presence* as flags (`admin_token_set: bool`,
  `env_managed`), so no secret value reaches a response or a log line.
- Every non-GET request (including `POST /login`, `PUT /api/config`, and
  `POST /api/logout`) requires `X-Requested-With: gh-release-notify` (CSRF
  defense with `SameSite=Strict`).
- `bind_addr`, `port`, `trust_proxy_auth`, and `state_path` are file/env-only;
  the UI rejects them as read-only.
- Config writes go through `config_writeback::apply`: `toml_edit` mutation,
  re-parse and `validate()` on the rendered bytes, then atomic tmp+rename. A
  validation failure leaves the target file untouched.
- The UI writes **only** the config path; `StateStore` ownership stays with
  the scheduler and `src/state.rs` is untouched by UI work.
- Runtime reload is `tokio::sync::watch<Arc<Config>>`: the scheduler re-reads
  each tick and re-arms its sleep on `config_rx.changed()`. Status flows the
  other way on `watch<Arc<StatusSnapshot>>`.
- Session ids come from `getrandom::fill`.

## Releases

Releases are cut locally via `./scripts/release.sh`; the tag push triggers
`.github/workflows/release.yml`. No `workflow_dispatch` trigger exists. Each
script's flags are in its `--help`.

- `scripts/release.sh` — infers the SemVer bump from conventional commits
  (breaking→major, feat→minor, else→patch) or takes `--major|--minor|--patch`;
  `--dry-run` previews. Pins git-cliff 2.14.1 and cargo-release 1.1.5, runs the
  gate, then cargo-release commits `chore: release v{version}`, tags
  `v{version}`, and pushes branch + tag.
- `scripts/release-rc.sh` — tag-only `vX.Y.Z-rc.N` (N starts at 0); refuses a
  dirty tree, non-`main` HEAD, or an existing tag. It never commits and never
  touches `Cargo.toml`/`Cargo.lock`/a version file.
- `scripts/backfill-release-notes.sh` — repairs GitHub Release bodies for
  existing tags.
- `scripts/test-container.sh` — podman smoke test (build, boot/poll, SIGTERM,
  invalid config); the state check is best-effort, and `-k` leaves the
  container up for UI inspection.
- `.github/workflows/release.yml` — `v*` tag only: gate → trivy fs scan
  (blocking HIGH/CRITICAL) → buildx amd64+arm64 push with SBOM + provenance →
  trivy image scan `--ignore-unfixed` (blocking) → SARIF → stable-only GitHub
  Release with a git-cliff body.

Invariants:

- Stable tags are `v{ver}`/`{ver}`/`latest`; RC tags are
  `v{ver}-rc.{n}`/`{ver}-rc.{n}`/`rc`. RC tags never render a `CHANGELOG.md`
  section (`cliff.toml` `tag_pattern` is stable-only) and never create a GitHub
  Release; their commits fold into the next stable release.
- **Lockstep toolchain pin**: CI toolchain (1.98.1) and Dockerfile builder
  (`rust:1.98.1-slim-trixie`) move together in one commit; builder and runtime
  stay on the same Debian release (both trixie) or the glibc-linked binary
  fails to start.
- **Fix forward**: on post-tag CI failure, no tag deletion, no revert — cut a
  new patch release.
- `release.toml` at repo root is the single source of truth for `cargo-release`
  behavior. Do NOT duplicate it under `[package.metadata.release]` in
  `Cargo.toml`.

## Testing approach

- Unit tests live in-module under `#[cfg(test)]`. Dev-deps: `tempfile` for
  filesystem tests, `tower` (`ServiceExt::oneshot`) for UI handler tests.
  `config.rs`, `config_writeback.rs`, and `ui/` carry most of the suite.
- No integration tests against real GitHub or real SMTP — both are thin
  wrappers over well-tested crates.
- The scheduler is glue; its behavior is exercised manually (smoke test at
  `poll_interval_seconds = 60`, the validation minimum), not unit-tested.

## SDD artifacts

- Base spec: `docs/superpowers/specs/2026-07-04-gh-release-notify-design.md`;
  plan: `docs/superpowers/plans/2026-07-04-gh-release-notify.md`.
- Cron: `docs/superpowers/specs/2026-07-10-cron-schedule-design.md`;
  plan: `docs/superpowers/plans/2026-07-10-cron-schedule.md`.
- Sender display name:
  `docs/superpowers/specs/2026-10-11-sender-display-name-design.md`; plan:
  `docs/superpowers/plans/2026-10-11-sender-display-name.md`.
- Web UI: `docs/SPEC-web-ui.md`, `docs/SPEC-config-writeback.md`,
  `docs/plan-web-ui.md`, capability map `docs/capability-map.md`; design and
  hardening plans under `docs/superpowers/plans/2026-10-20-web-ui-*.md`; binding
  visual contract `docs/UI-GUIDELINES.md`.
- Release workflow:
  `docs/superpowers/specs/2026-07-05-release-workflow-design.md` and its
  superseded plan; local flow
  `docs/superpowers/specs/2026-09-12-local-release-workflow-design.md` +
  `docs/superpowers/plans/2026-09-12-local-release-workflow.md`.
- Scratch: `.superpowers/sdd/<workstream>/` (git-ignored) holds `progress.md`,
  task briefs, and reports.
