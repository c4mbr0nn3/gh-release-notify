# gh-release-notify

A long-running Rust daemon that polls a configurable list of GitHub repos for new **stable** releases and sends plain-text email notifications via SMTP when one appears. Built for homelab use; ships as a Docker/podman container.

## What it does

- Polls `GET /repos/{owner}/{repo}/releases/latest` for each configured repo on a configurable schedule: a fixed interval (default 1h) or an optional cron expression for precise timing control.
- Compares the latest stable tag to the last-seen tag stored in a JSON state file.
- On a new release, sends a plain-text email (repo, tag, name, published date, URL, release notes) to a configurable recipient list via SMTP.
- On first run for a repo with no stored tag, records the current latest tag **without** emailing (so deploying the service doesn't spam you about the current release).
- On SMTP failure, leaves state untouched so the email retries next tick.
- Optional GitHub token for higher API rate limits (60 req/hour/IP unauthenticated, 5000 req/hour with token).
- Graceful shutdown on SIGINT/SIGTERM.

## Configure

Copy the sample config and edit:

```bash
cp config.example.toml config.toml
```

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

### Environment variables

| Variable        | Purpose                                                          | Default        |
|-----------------|------------------------------------------------------------------|----------------|
| `CONFIG_PATH`   | Path to the config file (also settable via `--config` CLI arg).  | `./config.toml`|
| `STATE_PATH`    | Overrides `state_path` from the config file (also settable via `--state-path` CLI arg). | (config value) |
| `GITHUB_TOKEN`  | Optional GitHub PAT. If set, sent as `Authorization: Bearer`.   | (unset)        |
| `SMTP_PASSWORD` | Overrides `[smtp].password`. Keeps the secret out of the file.  | (unset)        |
| `ADMIN_TOKEN`   | Admin token for the web UI login. Alternative to `[ui].admin_token`. | (unset)        |
| `RUST_LOG`      | tracing filter directive (`info`, `debug`, `gh_release_notify=debug`). | `info`    |

Copy `.env.example` to `.env` and fill in secrets:

```bash
cp .env.example .env
```

```
GITHUB_TOKEN=ghp_xxx
SMTP_PASSWORD=your-smtp-password
ADMIN_TOKEN=your-admin-token
```

## Run locally

```bash
cargo run --release -- --config ./config.toml
```

The shipped `config.example.toml` sets `state_path = "/state/state.json"`, which only exists inside the container. For a local run, either change `state_path` to `./state.json` in your config, or override it without editing the file:

```bash
cargo run --release -- --config ./config.toml --state-path ./state.json
# or:
STATE_PATH=./state.json cargo run --release -- --config ./config.toml
```

The override is runtime-only and never rewrites the config file, so the Docker value stays intact. State-saving failures (`No such file or directory`) mean the `state_path` parent directory does not exist.

For a quick smoke test set `poll_interval_seconds = 120` in the config and watch the logs.

## Run in Docker / podman

Build the image (use `docker` if available, otherwise `podman`):

```bash
docker build -t gh-release-notify:latest .
# or, if docker is not available:
podman build -t gh-release-notify:latest .
```

Run with `docker-compose.yml` (mounts `config.toml` read-write so the web UI can save edits, and a `./state` directory for persistence). Use whichever compose implementation you have available:

```bash
docker compose up -d        # docker compose plugin
# or:
podman compose up -d        # podman compose
# or:
docker-compose up -d        # standalone docker-compose
```

State persists across container restarts via the `./state` volume. On container recreation the state file survives, so you won't get a first-run notification burst for already-seen releases.

**State directory permissions:** the container runs as non-root user `ghrel` (uid 10001). The mounted `./state` directory must be writable by that uid, or you'll see `Permission denied (os error 13)` on state save. Before first run:

```bash
mkdir -p ./state && sudo chown 10001:10001 ./state
```

(If you see `failed to write state tmp file ... Permission denied` in the logs, this is the fix.)

## Web UI

The daemon serves a small web UI for status and configuration, embedded in the binary (no build step, no npm). By default it listens on `127.0.0.1:8080`.

- `/` — status page (watched repos and last-seen tags, last/next poll) plus a settings form.
- `/healthz` — plain JSON health check (public).

Config edits from the UI take effect **without a restart** (the scheduler re-reads the config on the next tick). Edits made by hand to `config.toml` require a restart.

### Auth modes

Access is gated by **exactly one** of three modes, resolved at startup:

1. **Admin token** — set `[ui].admin_token` in `config.toml`, or (preferred) the `ADMIN_TOKEN` environment variable. The UI shows a login form; the token is exchanged for an in-memory session cookie (12h expiry, `HttpOnly; SameSite=Strict`, `Secure` behind an HTTPS proxy). A restart logs you out.
2. **Trusted proxy** — set `ui.trust_proxy_auth = true`. The UI accepts the `Remote-User` header set by your reverse proxy, with no login page.
3. **Open** — no token and no proxy trust. Anyone who can reach the port can edit the config. Only acceptable on a trusted network.

Configuring a token **and** `trust_proxy_auth = true` together is a **startup error** — the daemon refuses to start rather than run with an ambiguous security posture.

Generate a token with:

```bash
openssl rand -hex 32
```

### Reverse proxy

The UI binds `127.0.0.1` by default, so it is not reachable from other hosts until you expose it. To put it behind a reverse proxy:

- Route the proxy to `gh-release-notify:8080` on the `gh-release-notify-net` network.
- For header-based auth (Authelia, Pangolin, etc.), set `trust_proxy_auth = true` and have the proxy set `Remote-User`. Note this is **spoofable if the UI port is directly reachable**, so make sure only the proxy can reach it.
- The shipped `docker-compose.yml` maps the port to `127.0.0.1:8080` on the host and sets `bind_addr = "0.0.0.0"` inside the container (container loopback is not reachable from the host). Keep the host mapping on loopback unless you intend LAN exposure.

### Read-only config

If the config file is mounted or permissioned read-only, the UI detects this, shows a banner, and disables saving. Mounting `./config.toml:/config/config.toml:ro` is a supported hardening option.

### Security posture

The UI can rewrite the file that holds the SMTP password, so in a read-write container deployment an admin token or proxy auth is effectively **mandatory**. No secret value is ever returned by any endpoint or written to the logs. Binding to `0.0.0.0` with no token logs a warning at startup. TLS is terminated by your reverse proxy; the daemon does not do TLS itself.

## First-run behavior

For each repo, the first time the service sees it (no stored tag in `state.json`) it records the current latest tag **without** sending an email. Subsequent new releases trigger an email. Delete `state.json` to reset.

## Logs

Structured logs via `tracing`. Default level `info`:

```
polling 2 repos
no change for fosrl/pangolin (still 1.19.4)
no change for fosrl/newt (still 1.13.0)
tick complete, sleeping 3600s
```

New release:

```
new release detected for fosrl/pangolin: 1.20.0
sent notification to 2 recipients for fosrl/pangolin 1.20.0
state saved to /state/state.json
```

Rate-limited (403):

```
github rate-limited, skipping remaining repos this tick: github rate-limited (403) for fosrl/pangolin: ...
rate-limited by github, skipping remaining repos this tick
```

Set `RUST_LOG=debug` for more detail.

## Project layout

```
src/
  main.rs        CLI, tracing, wire modules, config/status watch channels, signal handling
  config.rs      Config + SmtpConfig + UiConfig + Encryption + AuthMode: parse & validate config.toml
  github.rs      GithubClient + Release + GithubError: fetch latest stable release
  state.rs       StateStore: JSON-backed last-seen tags (atomic save)
  notify.rs      Mailer + build_body: plain-text email via SMTP (lettre async)
  scheduler.rs   run(): poll loop, first-run-no-email, graceful shutdown
  config_writeback.rs  ConfigEdit + SaveError + apply(): comment-preserving atomic config writes
  ui/mod.rs      axum router, AppState, handlers, tests
  ui/auth.rs     auth middleware, sessions, login limiter
  ui/assets.rs   include_str! constants for the embedded page
  ui/index.html  embedded UI page
  ui/app.js      embedded client logic
  ui/pico.classless.min.css  vendored Pico CSS 2.1.1 (MIT)
config.example.toml   sample config (with comments)
.env.example          sample env file
Dockerfile            multi-stage build (rust:slim -> debian:trixie-slim)
docker-compose.yml     single service, config + state volumes
```

## Develop

Verification gate (run before committing):

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

Run a focused test while iterating:

```bash
cargo test config
cargo test state
cargo test github
cargo test notify
cargo test ui
```

Build the release binary:

```bash
cargo build --release
```

## Releases

Releases are cut locally via `./scripts/release.sh` and pushed as a tag;
the tag push triggers the GitHub Actions workflow that builds and
publishes the image and the GitHub Release.

### Cutting a release

1. From a clean `main`, run `./scripts/release.sh` with no flags to infer
   the SemVer bump from conventional commits (breaking→major, feat→minor,
   else→patch), or pass `--major`, `--minor`, or `--patch` explicitly.
   Use `--dry-run` to preview.
2. The script enforces the pinned tools (git-cliff 2.14.1,
   cargo-release 1.1.5) and runs the verification gate (`cargo fmt`,
   `cargo clippy -- -D warnings`, `cargo test`).
3. It then bumps `Cargo.toml` and `Cargo.lock`, generates the changelog,
   commits as `chore: release v{version}`, tags `v{version}`, and pushes
   `main` + the tag.
4. The tag push triggers the `release` workflow, which builds and pushes
   a multi-arch (amd64 + arm64) Docker image to GHCR and creates a
   GitHub Release with notes generated by git-cliff.

### Image tags

The image is published to `ghcr.io/c4mbr0nn3/gh-release-notify` with
three tag flavors for each release `vX.Y.Z`:

- `:vX.Y.Z` — git tag verbatim. Most explicit; use for pinning.
- `:X.Y.Z` — SemVer without the `v` prefix. For tooling that strips `v`.
- `:latest` — points at the most recent release. Updated only on tag
  pushes, never on `main` branch pushes.

### Pulling the image

`docker-compose.yml` is configured to pull from GHCR by default:

```bash
docker compose pull
docker compose up -d
```

To pin a specific release, edit `docker-compose.yml`:

```yaml
    image: ghcr.io/c4mbr0nn3/gh-release-notify:v0.1.0
```

For local development builds, use `docker compose up --build` (the
`build: .` directive is retained as a fallback).

## License

Licensed under the [MIT License](LICENSE).

### Third-party assets

The web UI embeds **Pico CSS v2.1.1** (`pico.classless.min.css`), copyright 2019-2025, licensed under the MIT License.