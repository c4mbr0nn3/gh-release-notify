# gh-release-notify

A long-running Rust daemon that polls a configurable list of GitHub repos for new **stable** releases and sends plain-text email notifications via SMTP when one appears. Built for homelab use; ships as a Docker/podman container.

## What it does

- Polls `GET /repos/{owner}/{repo}/releases/latest` for each configured repo at a configurable interval (default 1h).
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
| `GITHUB_TOKEN`  | Optional GitHub PAT. If set, sent as `Authorization: Bearer`.   | (unset)        |
| `SMTP_PASSWORD` | Overrides `[smtp].password`. Keeps the secret out of the file.  | (unset)        |
| `RUST_LOG`      | tracing filter directive (`info`, `debug`, `gh_release_notify=debug`). | `info`    |

Copy `.env.example` to `.env` and fill in secrets:

```bash
cp .env.example .env
```

```
GITHUB_TOKEN=ghp_xxx
SMTP_PASSWORD=your-smtp-password
```

## Run locally

```bash
cargo run --release -- --config ./config.toml
```

For a quick smoke test set `poll_interval_seconds = 120` in the config and watch the logs.

## Run in Docker / podman

Build the image (use `docker` if available, otherwise `podman`):

```bash
docker build -t gh-release-notify:latest .
# or, if docker is not available:
podman build -t gh-release-notify:latest .
```

Run with `docker-compose.yml` (mounts `config.toml` read-only and a `./state` directory for persistence). Use whichever compose implementation you have available:

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

## First-run behavior

For each repo, the first time the service sees it (no stored tag in `state.json`) it records the current latest tag **without** sending an email. Subsequent new releases trigger an email. Delete `state.json` to reset.

## Logs

Structured logs via `tracing`. Default level `info`:

```
polling 2 repos
fetching latest for fosrl/pangolin
no change for fosrl/pangolin (still 1.19.4)
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
  main.rs        CLI, tracing, wire modules, signal handling
  config.rs      Config + SmtpConfig + Encryption: parse & validate config.toml
  github.rs      GithubClient + Release + GithubError: fetch latest stable release
  state.rs       StateStore: JSON-backed last-seen tags (atomic save)
  notify.rs      Mailer + build_body: plain-text email via SMTP (lettre async)
  scheduler.rs   run(): poll loop, first-run-no-email, graceful shutdown
config.example.toml   sample config (with comments)
.env.example          sample env file
Dockerfile            multi-stage build (rust:slim -> debian:bookworm-slim)
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
```

Build the release binary:

```bash
cargo build --release
```

## Design & plan

- Spec: `docs/superpowers/specs/2026-07-04-gh-release-notify-design.md`
- Implementation plan: `docs/superpowers/plans/2026-07-04-gh-release-notify.md`
- Project conventions: `AGENTS.md`

## Releases

Releases are cut manually from the GitHub Actions UI and are fully
reproducible — no local tooling or tokens required.

### Cutting a release

1. Go to **Actions → release → Run workflow** in the GitHub repo.
2. Choose the SemVer bump level (`patch`, `minor`, or `major`).
3. The `release` workflow runs the verification gate (`cargo fmt`,
   `cargo clippy -- -D warnings`, `cargo test`).
4. The workflow runs `cargo-release`, which bumps `Cargo.toml` and
   `Cargo.lock`, commits as `chore: release v{version}`, tags
   `v{version}`, and pushes to `main`.
5. The workflow builds and pushes a multi-arch (amd64 + arm64) Docker
   image to GHCR.
6. The workflow creates a GitHub Release with auto-generated notes
   derived from commits since the last tag.

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