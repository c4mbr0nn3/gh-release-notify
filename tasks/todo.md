# Web UI — Task List

Specs: `docs/SPEC-config-writeback.md`, `docs/SPEC-web-ui.md`
Plan: `docs/plan-web-ui.md`
Capability map: `docs/capability-map.md`

Tasks are ordered by dependency. Phase A must land before Phase B; Phase C is
independent of B once A and B are done. Mark a task `[x]` only after its own
verification steps pass.

## Phase A — config-writeback

- [ ] A1 — Add `toml_edit` dependency
- [ ] A2 — Add `config_path` to `Config`, `Serialize` to `Encryption`
- [ ] A3 — Add `SaveError` and `ConfigEdit` types
- [ ] A4 — Implement `config_writeback::apply()`
- [ ] A5 — Cover the unwritable-path error mapping

## Phase B — web-ui

- [ ] B1 — Add `axum`, `subtle`, `getrandom`, dev-dep `tower`
- [ ] B2 — Add `[ui]` config section and exclusive auth-mode validation
- [ ] B3 — Scheduler reloads config from `watch` and publishes status
- [ ] B4 — Auth middleware: sessions, login limiter, three modes
- [ ] B5 — `AppState`, router, server bootstrap
- [ ] B6 — Public handlers: static, health, login
- [ ] B7 — Config read/write and logout endpoints
- [ ] B8 — Frontend assets (Pico vendored, page, client JS)
- [ ] B9 — Wire UI server and reload channels into `main`

## Phase C — deployment and docs

- [ ] C1 — `config.example.toml` and `.env.example`
- [ ] C2 — `docker-compose.yml` rw mount + loopback port
- [ ] C3 — `README.md` UI, auth, security posture, third-party notice
- [ ] C4 — `AGENTS.md` scope rules and new Web UI section
- [ ] C5 — Final verification gate (incl. container build)

## Post-implementation

- [ ] Revisit `AGENTS.md` with the `writing-for-agents` skill
- [ ] Confirm no secret appears in any log line or response during the smoke test

## Standing constraints (every task)

- No comments in Rust source; no `unwrap`/`expect`/`panic` in non-test code.
- Never add `rand` — session entropy is `getrandom::fill`.
- Never return or log a secret value.
- The UI writes only the config path, never `state_path`.
- `src/state.rs` must not be modified.
- Gate before every commit: `cargo fmt && cargo clippy -- -D warnings && cargo test`.
