# Release workflow design — 2026-07-05

Semantic-versioning tag workflow + tag-triggered GitHub Action that runs the
project verification gate and publishes a multi-arch Docker image to GitHub
Container Registry.

## Goals

- Manual-but-reproducible releases triggered from the Actions UI (no local
  token juggling).
- `cargo-release` bumps `Cargo.toml` + `Cargo.lock`, commits, tags `v{version}`,
  and pushes.
- Tag push triggers a CI workflow that runs the same verification gate the
  developer runs locally (`cargo fmt`, `cargo clippy -- -D warnings`,
  `cargo test`) and then builds/pushes a multi-arch Docker image to GHCR.
- A GitHub Release with auto-generated notes is created for each tag.

## Non-goals

- Automated release on every push to `main` (manual trigger only).
- crates.io publishing (binary app, no publish step).
- Chelog file generation beyond GitHub's `generate_release_notes`.
- Pre-release / RC tag handling.

## Tooling

- `cargo-release` (pinned major) installed in the `release` job; cached across
  runs.
- `Swatinem/rust-cache@v2` for cargo registry + target cache.
- `softprops/action-gh-release@v2` for GitHub Release creation.
- `docker/setup-qemu-action@v3`, `docker/setup-buildx-action@v3`,
  `docker/login-action@v3`, `docker/metadata-action@v5`,
  `docker/build-push-action@v6` for the image pipeline.

## cargo-release configuration

Lives at repo root as `release.toml` (single source of truth; not embedded in
`Cargo.toml`):

```toml
allow-branch = ["main"]
sign-tag = false
push-remote = "origin"
consolidate-commits = true
pre-release-commit-message = "chore: release v{{version}}"
tag = true
tag-name = "v{{version}}"
tag-message = "chore: release v{{version}}"
push = true
publish = false
verify = true
```

`cargo-release` is responsible for bump + commit + tag + push. GitHub Release
creation is delegated to the workflow (next section) so cargo-release's role
stays narrow and the workflow owns the GitHub artifact.

## Workflow 1: `.github/workflows/release.yml`

**Trigger:** `workflow_dispatch` with input `level` (choice: `patch|minor|major`,
default `patch`). Run from Actions UI.

**Permissions:** `contents: write` (push tag + commit; create GitHub Release).

**Concurrency:** `group: release`, `cancel-in-progress: false` — never cancel
mid-release.

**Job `release`** on `ubuntu-latest`:

1. `actions/checkout@v4` with `fetch-depth: 0` (cargo-release needs full history
   to find the last tag).
2. `dtolnay/rust-toolchain@stable` (stable toolchain; matches Dockerfile's
   `1.86`-ish baseline).
3. `Swatinem/rust-cache@v2`.
4. Cache `~/.cargo/bin` so `cargo install cargo-release` is cached across runs.
5. `cargo install cargo-release --version ^0.25 --locked` (pinned major,
   locked deps).
6. Configure git: `user.name = github-actions[bot]`, `user.email =
   41898282+github-actions[bot]@users.noreply.github.com`.
7. `cargo release --no-similar-publish ${{ inputs.level }}` — bumps Cargo.toml
   + Cargo.lock, commits, tags `v{{version}}`, pushes.
8. Extract version from the pushed tag (parse `refs/tags/vX.Y.Z`).
9. `softprops/action-gh-release@v2` with `tag_name: v${{ version }}` and
   `generate_release_notes: true`.

## Workflow 2: `.github/workflows/ci.yml`

**Trigger:** `on.push.tags: ["v*.*.*"]` only.

**Concurrency:** `group: ci-${{ github.ref }}`, `cancel-in-progress: false`.

### Job `test` — `ubuntu-latest`, `permissions: { contents: read }`

Runs the AGENTS.md verification gate verbatim:

1. `actions/checkout@v4`.
2. `dtolnay/rust-toolchain@stable`.
3. `Swatinem/rust-cache@v2`.
4. `cargo fmt --check`.
5. `cargo clippy -- -D warnings`.
6. `cargo test`.

### Job `image` — `ubuntu-latest`, `permissions: { contents: read, packages: write }`, `needs: [test]`

1. `actions/checkout@v4`.
2. `docker/setup-qemu-action@v3` (arm64 emulation).
3. `docker/setup-buildx-action@v3`.
4. `docker/login-action@v3` with `registry: ghcr.io`, `username:
   ${{ github.actor }}`, `password: ${{ secrets.GITHUB_TOKEN }}`.
5. `docker/metadata-action@v5` with `images:
   ghcr.io/c4mbr0nn3/gh-release-notify`, `tags`:
   - `type=ref,event=tag` → `:v0.1.0` (git tag verbatim)
   - `type=semver,pattern={{version}}` → `:0.1.0` (semver without `v`)
   - `type=raw,value=latest,enabled=${{ startsWith(github.ref, 'refs/tags/') }}`
     → `:latest` only on tag pushes
6. `docker/build-push-action@v6` with `platforms: linux/amd64,linux/arm64`,
   `push: true`, `tags: ${{ steps.meta.outputs.tags }}`,
   `labels: ${{ steps.meta.outputs.labels }}`,
   `cache-from: type=gha`, `cache-to: type=gha,mode=max`.

## Image tag policy

- `:v0.1.0` — git tag verbatim. Pinning-friendly; unambiguous about the release.
- `:0.1.0` — semver without `v`. Convention for tooling that strips the `v`.
- `:latest` — only on tag pushes. Owned by the most recent release. NOT updated
  on `main` branch pushes (we don't run CI on `main`).

## docker-compose.yml update

Change `image:` from the local name to the registry image, keeping `build: .`
as fallback so `docker compose up --build` still works for local dev:

```yaml
services:
  gh-release-notify:
    image: ghcr.io/c4mbr0nn3/gh-release-notify:latest
    build: .
```

This lets homelab users `docker compose pull && docker compose up -d` to fetch
the CI-built image without building locally.

## Documentation updates

- **README.md** — new "Releases" section: how to cut a release from the Actions
  UI, what happens (release.yml → tag push → ci.yml → image), the three image
  tag flavors, how to pin in compose.
- **AGENTS.md** — new "Releases" subsection under "Commit conventions" (or its
  own section): the two workflows exist; `cargo-release` is invoked in CI, not
  locally; `ci.yml` runs the full verification gate (fmt/clippy/test) on tag
  push; `release.yml` runs `cargo test` only (via `verify = true`) as a
  pre-tag sanity check — the authoritative gate is `ci.yml`.

## Permissions and repo settings

- Each workflow declares an explicit `permissions:` block, so the repo-level
  default token permission setting does not need to be changed. The default
  `GITHUB_TOKEN` is sufficient; no PAT required.
- `release.yml` requires `contents: write` (push tag/commit + create release).
- `ci.yml` `image` job requires `packages: write` (push to GHCR).
- The published GHCR package inherits visibility from the repo on first
  push; if the repo is public, the package is public. If private, the package
  is private and homelab hosts need a `docker login ghcr.io` with a PAT that
  has `read:packages`.

## Out of scope

- `release-plz` / automated PR-style releases (deferred; manual trigger chosen).
- Multi-arch beyond amd64 + arm64.
- Build attestations / SBOM / cosign signing (deferred).
- Pre-release / RC tag handling.
- `:latest` on `main` pushes.