# Architecture

The binary runs two long-lived tokio tasks and connects them with watch
channels:

- the **scheduler** polls GitHub, sends email, and owns the state store;
- the **UI** serves the embedded page, reads status, and writes config.

```text
              config watch (Arc<Config>)
  UI PUT /api/config ──► config_writeback ──► scheduler
                                                │
                                                ├── GitHub API
                                                ├── SMTP
                                                └── StateStore (JSON)
                                                │
              status watch (Arc<StatusSnapshot>)│
  UI GET /api/config ◄───────────────────────────┘
```

## Module map

```text
src/
  main.rs              CLI, tracing, watch channels, UI/scheduler tasks, signals
  config.rs            Config, SmtpConfig, UiConfig, Encryption, AuthMode, validation
  github.rs            GithubClient, Release, GithubError
  state.rs             StateStore: JSON-backed last-seen tags, atomic save
  notify.rs            Mailer, build_body, sender formatting, plain-text email
  scheduler.rs         run(): poll loop, first-run handling, status publication
  config_writeback.rs  ConfigEdit, SaveError, apply(): comment-preserving atomic writes
  ui/mod.rs            axum router, AppState, handlers, tests
  ui/auth.rs           auth middleware, sessions, login limiter
  ui/assets.rs         include_str! constants for the embedded page
  ui/index.html        embedded UI page
  ui/app.js            embedded client logic
  ui/ui.css            embedded UI styles (no CSS framework)
config.example.toml    sample config
.env.example           sample environment file for Compose
Dockerfile             multi-stage build: rust:1.98.1-slim-trixie → debian:trixie-slim
docker-compose.yml     single service, config + state volumes
docs/                  user documentation and SDD artifacts
scripts/               release, RC, backfill, and container smoke-test scripts
```

## Data flow

### Polling and notification

1. The scheduler borrows the current `Arc<Config>` from its watch channel.
2. For each repo it calls `GET /repos/{owner}/{repo}/releases/latest`.
3. It compares the returned tag with `StateStore::last_seen`.
4. No tag stored → store the tag, no email. Same tag → no action.
   New tag → send the plain-text email, then store the tag.
5. A failed email leaves the tag unchanged; the next tick retries.
6. A 403 rate-limit response skips the remaining repos for that tick.
7. After the tick it publishes a `StatusSnapshot` and sleeps until the next
   interval or cron occurrence.

### Config reload

The UI calls `config_writeback::apply`, which mutates the TOML with `toml_edit`,
re-parses and validates the rendered bytes, then writes a temporary file and
renames it over the target. On success it sends the new `Arc<Config>` on the
config watch channel. The scheduler re-reads the config after its current tick
and recomputes the next poll. A failed validation leaves the file untouched.

### Status

The scheduler publishes repo tags, last poll time, and next poll time on a
second watch channel. `GET /api/config` combines that snapshot with the current
config and secret-presence flags. The UI polls this endpoint and refreshes after a
save.

## Boundaries

- **State ownership**: only the scheduler owns `StateStore`. The UI never reads or
  writes `state.rs`.
- **Config writes**: only `config_writeback::apply` writes the config file.
- **Secrets**: responses expose presence flags, never values. Logs never include
  the SMTP password or admin token.
- **No database**: state is a single JSON file; there is no migration layer.
- **No integration tests** against real GitHub or SMTP; both are thin wrappers over
  well-tested crates.

## Runtime and packaging

- Rust 2021, tokio async runtime.
- Single binary, no runtime assets: the UI is embedded with `include_str!`.
- The Docker builder pin (`rust:1.98.1-slim-trixie`) and the CI toolchain pin
  (`1.98.1`) move in lockstep. Builder and runtime stay on the same Debian
  release (trixie) so the glibc-linked binary starts.
- The runtime image runs as uid 10001 (`ghrel`) with no extra capabilities.
