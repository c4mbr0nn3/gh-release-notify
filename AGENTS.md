# AGENTS.md

## Project

gh-release-notify: a long-running Rust async daemon that polls a configurable list of GitHub repos for new stable releases and sends plain-text email notifications via SMTP. Deploys as a Docker/podman container for homelab use.

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

For release/deploy checks also run `cargo build --release`. Container tooling is **docker-first, podman-fallback**: when a container command is needed, check for `docker` first; if `docker` is not available on the system, fall back to `podman` (e.g. `podman build`). If neither `docker` nor `podman` is available, note container builds as not testable here. For compose, always probe for an available implementation first — check `docker compose` (plugin) and `podman compose` (or `docker-compose` standalone) and use whichever is present. `docker-compose.yml` is provided; if no compose implementation is available, note it as not testable here.

## Rules

- **No comments in Rust source files** unless explicitly requested. The `config.example.toml`, `.env.example`, `Dockerfile`, and `docker-compose.yml` are documentation files and MAY contain comments.
- **No `unwrap`/`expect`/`panic` in non-test code.** The `Regex::new(...).unwrap()` calls in `config.rs` and the `expect("install SIGTERM handler")` in `main.rs::unix_sigterm` are deliberate, plan-mandated exceptions (compile-time-constant patterns / fatal-startup path).
- **All fallible operations return `Result`.** Handle with `?` at task boundaries or log-and-continue; never swallow errors silently.
- **Out of scope (do NOT add):** HTML email, pre-release notifications, retry/backoff beyond "try again next tick", DB-backed state, per-repo stable/pre-release override.
- **State semantics:** first run for a repo with no stored tag stores the tag WITHOUT sending email. On SMTP failure, do NOT update state (retry email next tick). State save is atomic (write `<path>.tmp` then rename).

## Config & env

- Config file: TOML, path via `--config` CLI arg or `CONFIG_PATH` env, default `./config.toml`.
- Env overrides: `SMTP_PASSWORD` overrides `[smtp].password`; `GITHUB_TOKEN` (if set and non-empty) used for bearer auth; `RUST_LOG` controls tracing filter (default `info`).
- For Docker/compose deployment `state_path` must be `/state/state.json` (the mounted volume), not `./state.json`.
- Optional `cron_expression` in config: standard 5-field cron expression (UTC) that takes precedence over `poll_interval_seconds` when present. Auto-prepends seconds field for the `cron` crate. Day-of-week: 1=Sunday .. 7=Saturday.
- `[ui]` section (optional): `bind_addr` (default `127.0.0.1`), `port`
  (default `8080`), `admin_token` (default empty = no login),
  `trust_proxy_auth` (default false). `bind_addr`/`port` are read-only in
  the UI.
- `ADMIN_TOKEN` env var overrides `[ui].admin_token`. Leaving both unset
  means the UI runs with no login (open mode).
- Web UI deps: `axum`, `subtle` (constant-time compare), `getrandom`
  (session entropy), `toml_edit` (comment-preserving config writes);
  dev-dep `tower` (`ServiceExt::oneshot` for router tests).

## Web UI

An axum 0.8 server embedded in the daemon. The page is a single
`src/ui/index.html` with vanilla JS (`src/ui/app.js`) and vendored Pico CSS
2.1.1, all included with `include_str!` via `src/ui/assets.rs` — no JS build
step, no npm, no separate frontend artifact.

Invariants:

- Exactly one auth mode at startup: **token** (session cookie + optional
  `Authorization: Bearer`), **proxy** (`Remote-User` header, spoofable if the
  port is directly reachable), or **open**. `admin_token` and
  `trust_proxy_auth = true` together is a **startup error**.
- No secret value is ever returned by any endpoint or written to a log.
  `GET /api/config` exposes only `admin_token_set: bool` and `env_managed`
  flags, never values.
- Mutations (`PUT /api/config`, `POST /api/logout`) require
  `X-Requested-With: gh-release-notify` (CSRF defense with `SameSite=Strict`).
- `bind_addr` and `port` are file/env-only, never UI-editable.
- Config writes go through `config_writeback::apply`: `toml_edit` mutation,
  re-parse and `validate()` on the rendered bytes, then atomic tmp+rename.
  Validation failure never touches the target file.
- The UI's only filesystem write target is the config path. It never writes
  `state_path`.
- Runtime reload is `tokio::sync::watch<Arc<Config>>`: the scheduler re-reads
  each tick and re-arms its sleep on `config_rx.changed()`. Status flows the
  other way on `watch<Arc<StatusSnapshot>>`; `StateStore` ownership is
  unchanged and `src/state.rs` is not modified by UI work.
- Session ids come from `getrandom::fill`, never `rand`.
- **Post-implementation review pending:** this section should be revisited with
  the `writing-for-agents` skill now that the implementation has landed.

## Releases

Releases are cut locally via `./scripts/release.sh` (flags: `--dry-run`,
`--major`, `--minor`, `--patch`); a tag push then triggers a
tag-driven GitHub Actions workflow that builds and publishes the image and
the GitHub Release. No `workflow_dispatch` trigger exists.

- `scripts/release.sh` — local entry point. Infers the SemVer bump from
  conventional commits via git-cliff defaults (breaking→major, feat→minor,
  else→patch), or takes an explicit `--major|--minor|--patch`; `--dry-run`
  previews. It enforces exact pinned tools (git-cliff 2.14.1,
  cargo-release 1.1.5), runs the verification gate, then cargo-release
  makes the release commit `chore: release v{version}` (Cargo.toml +
  Cargo.lock + CHANGELOG.md via pre-release-hook), tags `v{version}`, and
  pushes branch + tag.
- `.github/workflows/release.yml` — triggered only by `v*` tag push:
  gate → trivy fs scan (blocking HIGH/CRITICAL) → buildx multi-arch
  (amd64 + arm64) image push to `ghcr.io/c4mbr0nn3/gh-release-notify`
  (tags `v{ver}`/`{ver}`/`latest`, SBOM + provenance) → trivy image scan
  `--ignore-unfixed` (blocking HIGH/CRITICAL) → SARIF uploads → installs
  git-cliff 2.14.1 → GitHub Release with a git-cliff-generated body.

- **Lockstep toolchain-pin rule**: CI toolchain pin (1.98.1) and Dockerfile
  builder pin (`rust:1.98.1-slim-trixie`) are bumped together in one commit,
  always. The builder and runtime stages must stay on the same Debian
  release (both trixie) or the glibc-linked binary fails to start at runtime.
- **Fix-forward rule**: on post-tag CI failure, no tag deletion, no revert
  dance — fix forward on a new patch release.

### Verification gate

The script runs the full gate (`cargo fmt`, `cargo clippy -- -D warnings`,
`cargo test`) locally before executing cargo-release, and `release.yml`
runs the same gate again on tag push before building the image.

### Release config

`release.toml` at repo root is the single source of truth for
`cargo-release` behavior. Do NOT duplicate these settings under
`[package.metadata.release]` in `Cargo.toml`.

## Testing approach

- Unit tests live in-module under `#[cfg(test)]`. Dev-dep: `tempfile` for filesystem tests.
- No integration tests against real GitHub or real SMTP — both are thin wrappers over well-tested crates.
- The scheduler is glue; its behavior is exercised manually (smoke test with short `poll_interval_seconds`), not unit-tested.
- Run a focused test while iterating: `cargo test config` / `cargo test state` / etc. Run the full suite once before committing.

## SDD artifacts

- Spec: `docs/superpowers/specs/2026-07-04-gh-release-notify-design.md`
- Plan: `docs/superpowers/plans/2026-07-04-gh-release-notify.md`
- Release-workflow spec: `docs/superpowers/specs/2026-07-05-release-workflow-design.md`
- Release-workflow plan: `docs/superpowers/plans/2026-07-05-release-workflow.md` (superseded)
- Local release-workflow spec: `docs/superpowers/specs/2026-09-12-local-release-workflow-design.md`
- Local release-workflow plan: `docs/superpowers/plans/2026-09-12-local-release-workflow.md`
- Progress ledger: `.superpowers/sdd/progress.md` (git-ignored scratch; recover from `git log` if destroyed).
- Per-task briefs and reports live under `.superpowers/sdd/`.
