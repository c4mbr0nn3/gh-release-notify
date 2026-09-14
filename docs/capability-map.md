# Capability Map: Web UI

**Date:** 2026-09-14
**Status:** Approved (design grilling session, 2026-09-14)

## Initiative

Add a lightweight web UI served by the existing `gh-release-notify` Rust
daemon, so a single operator can inspect daemon status and edit configuration
without hand-editing `config.toml` and restarting the container.

## Module decomposition

| Module id | Spec | Objective | Risk profile | Verification owner |
|---|---|---|---|---|
| `web-ui` | `docs/SPEC-web-ui.md` | Serve the embedded SPA, gate it, expose config read/write and status/health endpoints | HTTP + auth surface: an endpoint that is accidentally public is a config-rewriting endpoint | `src/ui/mod.rs` tests (`tower::ServiceExt::oneshot`) |
| `config-writeback` | `docs/SPEC-config-writeback.md` | Mutate `config.toml` in place, preserving comments, with validate-before-rename atomicity | Destructive filesystem mutation: a bug corrupts the mounted config volume | `src/config.rs` tests (`tempfile`) |

Modules are separate because their invariants and test strategies do not
overlap. `config-writeback` has no HTTP dependency and is testable with no
server running; `web-ui` has no filesystem-write dependency and is testable
with no config file on disk.

## Dependency order

```
config-writeback  ──►  web-ui
   (write path)         (calls the write path)
```

`web-ui` consumes `config_writeback::save(...)`; nothing in
`config-writeback` depends on `web-ui`. Implementation order follows this
arrow.

## Shared decisions (both modules)

These are settled once and referenced by both specs.

| Decision | Choice |
|---|---|
| HTTP framework | `axum` 0.8.9 (tokio-native; the only genuinely new package group) |
| Frontend | Single self-contained HTML file, vanilla JS, no build step, embedded with `include_str!` |
| CSS | Pico CSS 2.1.1 `pico.classless.min.css`, vendored, MIT header retained |
| Constant-time compare | `subtle` (`ct_eq`) — 2.6.1 already in `Cargo.lock` via `reqwest`: +0 packages |
| Session entropy | `getrandom::fill` — 0.4.3 already in `Cargo.lock` via `tempfile`: +0 packages. No `rand` |
| Config editing crate | `toml_edit` (preserves comments and key order) |
| Test harness | `tower::ServiceExt::oneshot` — `tower` 0.5.3 already in `Cargo.lock` via `reqwest`: +0 packages |
| Runtime reload | `tokio::sync::watch` carrying `Arc<Config>` (zero new dependencies) |
| Status publication | Per-tick `Arc<StatusSnapshot>` on a `watch` channel; the scheduler keeps sole ownership of `StateStore` |
| Auth modes | Exactly one of: admin-token login, trusted proxy header, or open. Token + proxy together is a startup error |
| Secrets in responses | Never. No secret value appears in any `GET` payload, in any encoding |
| Secrets in logs | Never. Secrets log as a status word (`changed` / `cleared`) |
| UI filesystem writes | Only the config path. Never `state_path` |
| Release coupling | `feat`, inferred minor bump. No `release.yml` or `Dockerfile` changes |

## Out of scope (explicitly)

- Manual actions in the UI (force-poll-now, send-test-email).
- Per-repo intervals or per-repo cron.
- Hot file-watching of `config.toml` (manual edits require a restart).
- Multi-user accounts, roles, or per-user sessions.
- TLS termination in the daemon (the reverse proxy owns TLS).
- Compiled SPA toolchain, npm, or any JS build step.
- Datastar / SSE live updates (candidate for a future version).
