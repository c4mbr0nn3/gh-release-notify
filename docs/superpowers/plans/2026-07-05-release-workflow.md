# Release Workflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a manual-trigger release workflow (`release.yml`) that runs `cargo-release` to bump/tag/push, and a tag-triggered CI workflow (`ci.yml`) that runs the verification gate and builds/pushes a multi-arch Docker image to GHCR.

**Architecture:** Two independent GitHub Actions workflows. `release.yml` runs on `workflow_dispatch` (Actions UI), installs `cargo-release`, bumps `Cargo.toml`+`Cargo.lock`, tags `v{version}`, pushes, then creates a GitHub Release with auto-generated notes. The tag push triggers `ci.yml`, which runs fmt/clippy/test in a `test` job, then builds and pushes the multi-arch (amd64+arm64) image to `ghcr.io/c4mbr0nn3/gh-release-notify` in an `image` job that depends on `test`.

**Tech Stack:** GitHub Actions, `cargo-release` 1.x, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `softprops/action-gh-release@v2`, `docker/setup-qemu-action@v3`, `docker/setup-buildx-action@v3`, `docker/login-action@v3`, `docker/metadata-action@v5`, `docker/build-push-action@v6`.

## Global Constraints

- No comments in Rust source files (existing AGENTS.md rule; this plan touches no Rust source, so this is moot here, but the rule applies to any Rust that may be touched).
- `config.example.toml`, `.env.example`, `Dockerfile`, `docker-compose.yml`, and workflow YAML files are documentation/infra files and MAY contain comments.
- Conventional Commits only: `<type>: <subject>`, subject <= 72 chars, no body, imperative mood, no trailing period. Types: `feat`, `fix`, `docs`, `chore`, `build`, `refactor`, `test`.
- This plan touches only infra files (YAML, TOML config, docs). No Rust source changes. No `cargo` verification gate applies to the workflow YAML itself, but the gate (`cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`) MUST pass on the existing Rust source before any commit.
- Image name is `ghcr.io/c4mbr0nn3/gh-release-notify` (lowercase; matches the GitHub owner `c4mbr0nn3`).
- Workflow `permissions:` blocks are explicit per-job; no reliance on repo-wide default token settings. No PAT required — the default `GITHUB_TOKEN` is sufficient.

---

## File Structure

- Create: `.github/workflows/release.yml` — manual-trigger release workflow (cargo-release + GitHub Release creation).
- Create: `.github/workflows/ci.yml` — tag-triggered CI workflow (verification gate + Docker build/push).
- Create: `release.toml` — repo-root cargo-release config (single source of truth for release behavior). Pinned to `cargo-release` 1.x semantics (current major as of 2026-07).
- Modify: `docker-compose.yml:3` — change `image:` from `gh-release-notify:latest` to `ghcr.io/c4mbr0nn3/gh-release-notify:latest`.
- Modify: `README.md` — add "Releases" section documenting the release process and image tags.
- Modify: `AGENTS.md` — add "Releases" subsection documenting the two workflows and where cargo-release is invoked.

---

### Task 1: Add `release.toml` cargo-release config

**Files:**
- Create: `release.toml`

**Interfaces:**
- Produces: a cargo-release config consumed by Task 2's `cargo release --config release.toml` invocation. Keys are read by `cargo-release` 1.x; the `release.yml` workflow passes `--config release.toml` explicitly so the file is found regardless of cwd. `publish = false` disables crates.io publishing (no extra CLI flag needed).

- [ ] **Step 1: Verify the verification gate passes on current source**

Run from repo root:
```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
Expected: all three succeed (this is the AGENTS.md gate; it must be green before any commit in this repo).

- [ ] **Step 2: Create `release.toml`**

```toml
# cargo-release config for gh-release-notify.
# Invoked from .github/workflows/release.yml as:
#   cargo release --config release.toml --execute <level>
# This file is the single source of truth for release behavior; do NOT
# duplicate these settings under [package.metadata.release] in Cargo.toml.

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

- [ ] **Step 3: Sanity-check the config is valid TOML**

Run:
```bash
python3 -c "import tomllib; tomllib.load(open('release.toml','rb'))" && echo OK
```
Expected: prints `OK`. If python3 is unavailable, `cargo release --config release.toml --list patch` (dry-run list, no `--execute`) prints the resolved config without executing.

- [ ] **Step 4: Commit**

```bash
git add release.toml
git commit -m "chore: add cargo-release config"
```

---

### Task 2: Add `.github/workflows/release.yml`

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `release.toml` from Task 1.
- Produces: a git tag `v{version}` pushed to `origin/main`, and a GitHub Release attached to that tag. The tag push is the trigger for `ci.yml` (Task 3).

- [ ] **Step 1: Create the workflow directory**

```bash
mkdir -p .github/workflows
```

- [ ] **Step 2: Write `.github/workflows/release.yml`**

```yaml
# Manually-triggered release workflow.
# Run from Actions UI: Actions → release → Run workflow → choose level.
# Bumps Cargo.toml/Cargo.lock via cargo-release, commits, tags v{version},
# pushes, then creates a GitHub Release with auto-generated notes.
name: release

on:
  workflow_dispatch:
    inputs:
      level:
        description: "SemVer bump level"
        required: true
        type: choice
        default: patch
        options:
          - patch
          - minor
          - major

permissions:
  contents: write

concurrency:
  group: release
  cancel-in-progress: false

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - name: Checkout (full history)
        uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Rust toolchain
        uses: dtolnay/rust-toolchain@stable

      - name: Cache cargo registry + target
        uses: Swatinem/rust-cache@v2

      - name: Cache cargo-install bin dir
        uses: actions/cache@v4
        with:
          path: ~/.cargo/bin
          key: cargo-bin-cargo-release-1

      - name: Install cargo-release
        run: cargo install cargo-release --version '^1' --locked --force

      - name: Configure git
        run: |
          git config user.name  "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"

      - name: Run cargo-release
        id: cargo_release
        run: |
          cargo release --config release.toml --execute ${{ inputs.level }}
        # publish = false in release.toml disables crates.io publishing; no
        # extra flag is needed (and --no-similar-publish is not a real flag).

      - name: Extract version from pushed tag
        id: version
        run: |
          tag="${GITHUB_REF#refs/tags/}"
          # GITHUB_REF on workflow_dispatch is the branch, not the tag cargo-release
          # just pushed. cargo-release pushes the tag to origin; we fetch the
          # latest tag matching v*.*.* on the current commit instead.
          git fetch --tags --force
          version_tag="$(git describe --tags --exact-match HEAD)"
          echo "tag=$version_tag" >> "$GITHUB_OUTPUT"
          echo "version=${version_tag#v}" >> "$GITHUB_OUTPUT"

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          tag_name: ${{ steps.version.outputs.tag }}
          generate_release_notes: true
```

- [ ] **Step 3: Lint the YAML syntax**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))" && echo OK
```
Expected: prints `OK`. If python3/yaml unavailable, a basic structural check is: the file has `name:`, `on:`, `jobs:` keys at top level and no tab characters (`grep -P '\t' .github/workflows/release.yml` should print nothing).

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: add manual-trigger release workflow"
```

---

### Task 3: Add `.github/workflows/ci.yml`

**Files:**
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: the `v*.*.*` tag push produced by Task 2's `release.yml`.
- Produces: a pushed multi-arch Docker image at `ghcr.io/c4mbr0nn3/gh-release-notify` with tags `:v{version}`, `:{version}`, and `:latest`.

- [ ] **Step 1: Write `.github/workflows/ci.yml`**

```yaml
# Tag-triggered CI: runs the verification gate, then builds and pushes a
# multi-arch (amd64 + arm64) Docker image to GitHub Container Registry.
# Triggered by any tag push matching v*.*.* (produced by release.yml).
name: ci

on:
  push:
    tags:
      - "v*.*.*"

permissions:
  contents: read

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: false

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy

      - name: Cache cargo registry + target
        uses: Swatinem/rust-cache@v2

      - name: cargo fmt
        run: cargo fmt --check

      - name: cargo clippy
        run: cargo clippy -- -D warnings

      - name: cargo test
        run: cargo test

  image:
    runs-on: ubuntu-latest
    needs: [test]
    permissions:
      contents: read
      packages: write
    steps:
      - uses: actions/checkout@v4

      - name: Set up QEMU
        uses: docker/setup-qemu-action@v3

      - name: Set up Docker Buildx
        uses: docker/setup-buildx-action@v3

      - name: Login to GHCR
        uses: docker/login-action@v3
        with:
          registry: ghcr.io
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Image metadata
        id: meta
        uses: docker/metadata-action@v5
        with:
          images: ghcr.io/c4mbr0nn3/gh-release-notify
          tags: |
            type=ref,event=tag
            type=semver,pattern={{version}}
            type=raw,value=latest,enabled=${{ startsWith(github.ref, 'refs/tags/') }}

      - name: Build and push
        uses: docker/build-push-action@v6
        with:
          context: .
          platforms: linux/amd64,linux/arm64
          push: true
          tags: ${{ steps.meta.outputs.tags }}
          labels: ${{ steps.meta.outputs.labels }}
          cache-from: type=gha
          cache-to: type=gha,mode=max
```

- [ ] **Step 2: Lint the YAML syntax**

Run:
```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" && echo OK
```
Expected: prints `OK`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add tag-triggered test and image publish workflow"
```

---

### Task 4: Update `docker-compose.yml` image reference

**Files:**
- Modify: `docker-compose.yml:3` (the `image:` line under the `gh-release-notify` service)

**Interfaces:**
- Produces: a `docker-compose.yml` that pulls `ghcr.io/c4mbr0nn3/gh-release-notify:latest` by default while still supporting local builds via `docker compose up --build` (the `build: .` key is retained).

- [ ] **Step 1: Read the current `docker-compose.yml` to confirm the exact line**

```bash
sed -n '1,5p' docker-compose.yml
```
Expected output begins with `services:` then `  gh-release-notify:` then `    image: gh-release-notify:latest`.

- [ ] **Step 2: Edit `docker-compose.yml` — change the `image:` line**

Replace:
```yaml
    image: gh-release-notify:latest
```
with:
```yaml
    image: ghcr.io/c4mbr0nn3/gh-release-notify:latest
```
Leave the `build: .` line unchanged so `docker compose up --build` still works for local dev.

The resulting top of file should read:
```yaml
services:
  gh-release-notify:
    image: ghcr.io/c4mbr0nn3/gh-release-notify:latest
    build: .
```

- [ ] **Step 3: Verify compose can parse the file (if a compose implementation is available)**

Probe for a compose implementation:
```bash
( command -v docker >/dev/null && docker compose version ) \
  || ( command -v podman >/dev/null && podman compose version ) \
  || ( command -v docker-compose >/dev/null && docker-compose version ) \
  || echo "no compose implementation available — skip parse check"
```

If a compose implementation was found, validate config:
```bash
docker compose config -q 2>/dev/null || podman compose config -q 2>/dev/null || docker-compose config -q
```
Expected: exits 0 with no output. If no compose implementation is available, note it as not testable here and proceed (the YAML change is a one-line `image:` swap and was validated by reading the file).

- [ ] **Step 4: Commit**

```bash
git add docker-compose.yml
git commit -m "build: point compose at GHCR image"
```

---

### Task 5: Document the release workflow in `README.md`

**Files:**
- Modify: `README.md` (add a "Releases" section; placement is after the existing build/deploy content, before the License section if one exists)

**Interfaces:**
- Consumes: the workflows from Tasks 2 & 3 (documented behavior).
- Produces: user-facing docs explaining how to cut a release and how to pin an image tag.

- [ ] **Step 1: Read the current `README.md` to find the insertion point**

Run:
```bash
grep -n "^## " README.md
```
Note the section headings so the new "Releases" section lands after deploy/build instructions and before License/Contributing if present.

- [ ] **Step 2: Insert the "Releases" section**

Add this section (adjust heading level to match the existing README convention — use `## Releases` if the README uses `##` for top-level sections, `# Releases` if it uses `#`):

```markdown
## Releases

Releases are cut manually from the GitHub Actions UI and are fully
reproducible — no local tooling or tokens required.

### Cutting a release

1. Go to **Actions → release → Run workflow** in the GitHub repo.
2. Choose the SemVer bump level (`patch`, `minor`, or `major`).
3. The `release` workflow runs `cargo-release`, which bumps `Cargo.toml`
   and `Cargo.lock`, commits as `chore: release v{version}`, tags
   `v{version}`, and pushes to `main`.
4. The workflow then creates a GitHub Release with auto-generated notes
   derived from commits since the last tag.
5. The tag push triggers the `ci` workflow, which runs the verification
   gate (`cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`) and
   then builds and pushes a multi-arch (amd64 + arm64) Docker image to
   GHCR.

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
```

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -m "docs: document release workflow and image tags"
```

---

### Task 6: Document the release workflow in `AGENTS.md`

**Files:**
- Modify: `AGENTS.md` (add a "Releases" subsection)

**Interfaces:**
- Consumes: the workflows from Tasks 2 & 3.
- Produces: contributor-facing docs that constrain future changes to the release pipeline.

- [ ] **Step 1: Read the current `AGENTS.md` to find the insertion point**

Run:
```bash
grep -n "^## " AGENTS.md
```
The new "Releases" section should land after "Commit conventions" (or its own top-level section near the end, before "Testing approach" if that reads better).

- [ ] **Step 2: Insert the "Releases" section**

Add:

```markdown
## Releases

Releases are cut via two GitHub Actions workflows; `cargo-release` is
invoked in CI, not locally.

- `.github/workflows/release.yml` — `workflow_dispatch` with a
  `patch|minor|major` input. Installs `cargo-release` 1.x, runs it
  against `release.toml` (repo root), which bumps `Cargo.toml` +
  `Cargo.lock`, commits as `chore: release v{version}`, tags
  `v{version}`, pushes to `main`, then creates a GitHub Release with
  auto-generated notes.
- `.github/workflows/ci.yml` — triggered by `v*.*.*` tag pushes. Runs
  the verification gate (fmt/clippy/test) in a `test` job, then builds
  and pushes a multi-arch (amd64 + arm64) Docker image to
  `ghcr.io/c4mbr0nn3/gh-release-notify` in an `image` job (needs
  `test`).

### Verification gate in CI

`ci.yml` runs the full gate (`cargo fmt`, `cargo clippy -- -D warnings`,
`cargo test`) on tag push. `release.yml` runs `cargo test` only (via
`cargo-release`'s `verify = true`) as a pre-tag sanity check — the
authoritative gate is `ci.yml`.

### Release config

`release.toml` at repo root is the single source of truth for
`cargo-release` behavior. Do NOT duplicate these settings under
`[package.metadata.release]` in `Cargo.toml`.
```

- [ ] **Step 3: Commit**

```bash
git add AGENTS.md
git commit -m "docs: document release workflows in AGENTS.md"
```

---

### Task 7: Smoke-verify the workflows offline

**Files:** none modified.

**Interfaces:**
- Consumes: the two workflow files from Tasks 2 & 3 and `release.toml` from Task 1.
- Produces: confidence that the YAML parses and the cross-references between files are consistent.

- [ ] **Step 1: Re-run the verification gate on Rust source (unchanged, must still be green)**

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```
Expected: all pass.

- [ ] **Step 2: Validate both workflow YAML files parse**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))" && echo "release.yml OK"
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))" && echo "ci.yml OK"
python3 -c "import tomllib; tomllib.load(open('release.toml','rb'))" && echo "release.toml OK"
```
Expected: all three print `OK`.

- [ ] **Step 3: Cross-check the image name is consistent across files**

```bash
grep -Rn "ghcr.io/c4mbr0nn3/gh-release-notify" .github/ docker-compose.yml README.md AGENTS.md
```
Expected: matches in `.github/workflows/ci.yml`, `docker-compose.yml`, `README.md`, `AGENTS.md`. No other image names should appear.

- [ ] **Step 4: Cross-check the tag pattern is consistent**

```bash
grep -Rn 'v\*\.\*\.\*\|v{{version}}\|v{version}' .github/ release.toml README.md AGENTS.md
```
Expected: `ci.yml` trigger `v*.*.*`, `release.toml` `tag-name = "v{{version}}"`, README/AGENTS narrative `v{version}` / `vX.Y.Z`. No mismatched variants.

- [ ] **Step 5: No commit needed (validation only)**

If all checks pass, the implementation is complete. If any check fails, fix the offending file and amend the relevant task's commit per the receiving-code-review / verification-before-completion skills.