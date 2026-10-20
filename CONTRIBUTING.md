# Contributing

## Prerequisites

- Rust 1.98.1 (the CI and Dockerfile pin).
- Docker or podman for container builds.
- podman specifically for `./scripts/test-container.sh`.

## Local build and run

```bash
cp config.example.toml config.toml
# Set state_path = "./state.json" in config.toml, or override it at runtime:
cargo run --release -- --config ./config.toml --state-path ./state.json
```

`config.example.toml` uses `/state/state.json` for Docker. The `--state-path`
override is runtime-only and never rewrites the file.

The daemon does not load `.env`. Docker Compose reads it automatically; for a
local run, export the variables in your shell:

```bash
export SMTP_PASSWORD=...
export GITHUB_TOKEN=...
export RUST_LOG=debug
```

## Verification gate

Run before committing:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

Run focused tests while iterating, then the full suite once before committing:

```bash
cargo test config
cargo test config_writeback
cargo test ui
cargo test state
cargo test github
cargo test notify
```

The scheduler is glue and is not unit-tested; exercise it manually with a short
interval (`poll_interval_seconds = 60`, the validation minimum) and watch the
logs.

## Code rules

- **No comments in Rust source files** unless explicitly requested. Config
  examples, `.env.example`, `Dockerfile`, and `docker-compose.yml` are
  documentation and may contain comments.
- **No `unwrap`/`expect`/`panic` in non-test code.** The two `Regex::new(...)`
  calls in `config.rs` and the SIGTERM handler installation in `main.rs` are
  deliberate exceptions.
- **All fallible operations return `Result`.** Propagate with `?` or log and
  continue; do not swallow errors.
- No HTML email, pre-release notifications, retry/backoff beyond "try again next
  tick", database state, or per-repo stable/pre-release overrides.
- State semantics: first run stores without emailing; SMTP failure leaves state
  unchanged; state saves are atomic.

## Commits

Follow [Conventional Commits](https://www.conventionalcommits.org/):
`<type>: <subject>`, with `feat`, `fix`, `docs`, `chore`, `build`, `ci`,
`refactor`, or `test`. Keep the subject at or under 72 characters, imperative,
no trailing period. **No commit body.**

## Container changes

```bash
./scripts/test-container.sh
```

The script requires podman. It builds the test image, boots it against temporary
config and state directories, and checks config loading, polling, SIGTERM
shutdown, and invalid-config rejection. See
[Operations](docs/operations.md#container-smoke-test) for flags.

## More documentation

- [Configuration](docs/configuration.md)
- [Web UI & auth](docs/web-ui.md)
- [Operations](docs/operations.md)
- [Architecture](docs/architecture.md)
- [Releases](docs/releases.md)
