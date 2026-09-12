# Implementation Plan: Local-driven release workflow

Spec: `docs/superpowers/specs/2026-09-12-local-release-workflow-design.md`
(approved 2026-09-12)

## Overview

Replace the broken CI-side release (cargo-release inside a `workflow_dispatch`
workflow) with a local-first flow: `scripts/release.sh` infers the bump,
generates the changelog, gates, commits, tags, pushes; a tag-triggered CI
workflow re-gates, scans (repo pre-build, image post-push), builds/pushes the
multi-arch OCI image with attestations, and creates the GitHub Release with a
git-cliff body.

## Architecture decisions

- **Bump + changelog locally, artifacts in CI.** Irreversible steps are pushed
  to where they fail least: everything local is revertible until `git push`.
- **git-cliff defaults are the bump rules.** breaking→major, feat→minor,
  everything else→patch; nothing skipped. No `[bump]` config needed.
- **cargo-release owns the release commit** via
  `pre-release-hook = ["git","cliff","--unreleased","--tag","v{{version}}",
  "--prepend","CHANGELOG.md"]` — changelog lands in the same commit.
  Script's `--dry-run` never invokes cargo-release (the hook runs even under
  `cargo release --dry-run` — designed out).
- **Exact pins everywhere**: Rust 1.98.1 (CI + Dockerfile builder, lockstep
  commits), git-cliff 2.14.1, cargo-release 1.1.5, `--locked` installs only.
  Both tools are pinned globals; script enforces versions, installs nothing.
- **Trivy triage by ownership**: `trivy fs` blocks early on Cargo.lock
  advisories; `trivy image --ignore-unfixed` blocks post-push only on
  remediable findings; SARIF uploaded for visibility.
- **Fix-forward on post-tag CI failure** — accepted, documented, no revert path.

## Task List

### Phase 1: Local release machinery

- [ ] **Task 1: `cliff.toml` + seed `CHANGELOG.md`**
  - Acceptance: `cliff.toml` defines header/footer/Keep-a-Changelog template;
    `git cliff -o CHANGELOG.md` renders history v1.0.0→v1.0.2 cleanly
    (hand-check output once, then commit the generated file).
  - Verify: `git cliff -o CHANGELOG.md && git cliff --unreleased` exits 0.
  - Files: `cliff.toml`, `CHANGELOG.md`
  - Dependencies: None

- [ ] **Task 2: Rework `release.toml` for local mode**
  - Acceptance: local-mode config with `pre-release-hook` as above;
    `pre-release-commit-message = "chore: release v{{version}}"`; `publish =
    false`, `push = true`, `tag = true`; no CI-era leftovers; `--dry-run`
    note documented in file header.
  - Verify: `cargo release --dry-run --no-confirm patch` (after Task 1's
    commit is in; expect hook to touch CHANGELOG.md — revert with `git
    checkout CHANGELOG.md`).
  - Files: `release.toml`
  - Dependencies: Task 1

- [ ] **Task 3: `scripts/release.sh`**
  - Acceptance: `set -euo pipefail`; dep check (exact versions, prints pinned
    install cmds); safety rails (clean worktree / on main / pushed / tag-free
    `v{next}` / ≥1 commit since last tag); bump inference or `--major|--minor|
    --patch` override; hard local gate; dry-run path (no gate, no writes);
    execution path via cargo-release; fix-forward documented in header.
  - Verify: `bash -n scripts/release.sh`; `./scripts/release.sh --dry-run`
    ×3 (inferred, forced level, and a refusal case e.g. dirty worktree).
  - Files: `scripts/release.sh` (+ `chmod +x`)
  - Dependencies: Tasks 1–2

### Checkpoint: Local machinery
- [ ] `--dry-run` shows sensible version/level/changelog for current history
- [ ] Full local gate (`cargo fmt --check`, `cargo clippy -- -D warnings`,
      `cargo test`) passes on the repo

### Phase 2: CI + Dockerfile

- [ ] **Task 4: Dockerfile — exact builder pin**
  - Acceptance: `rust:1.98.1-slim` builder (exact); runtime stays floating
    `debian:bookworm-slim` with a comment explaining why; everything else
    unchanged.
  - Verify: `docker build .` (or podman fallback; else note not testable) and
    `cargo build --release` per AGENTS.md release checks.
  - Files: `Dockerfile`
  - Dependencies: None (parallel with Tasks 1–3)

- [ ] **Task 5: Rewrite `.github/workflows/release.yml` (tag-triggered)**
  - Acceptance: `on: push: tags: ['v*']`; job-scoped `contents: write` +
    `packages: write`; toolchain pinned exact `1.98.1`; gate → `trivy fs`
    (blocking HIGH/CRITICAL) → buildx multi-arch with `provenance`/`sbom` +
    metadata-action tags (`v{ver}`, `{ver}`, `latest`) + push → `trivy image
    --ignore-unfixed` (blocking HIGH/CRITICAL) → SARIF uploads (both scans) →
    `cargo install git-cliff --locked --version 2.14.1` → release body from
    `git cliff --unreleased --tag v{version}` → GitHub Release. No
    `workflow_dispatch`, no old steps.
  - Verify: actionlint if available, else YAML parse; step-by-step read
    against spec §CI workflow.
  - Files: `.github/workflows/release.yml`
  - Dependencies: Task 4 (image refs must match)

### Checkpoint: CI shape
- [ ] Workflow matches spec order exactly; permissions job-scoped only
- [ ] No floating tool versions anywhere in the workflow

### Phase 3: Docs + cleanup

- [ ] **Task 6: Documentation sweep**
  - Acceptance: superseded headers on `2026-07-05-release-workflow*` spec/plan
    pointing to the new spec; AGENTS.md Releases section rewritten for the
    local-script flow (incl. lockstep toolchain-pin rule and fix-forward);
    README release instructions updated if it mentions the old flow.
  - Verify: grep for stale references (`workflow_dispatch`, "cargo-release in
    CI") across docs/AGENTS/README → zero uncorrected hits.
  - Files: `AGENTS.md`, `README.md`, 2 old SDD docs
  - Dependencies: Tasks 1–5 (describes final state)

- [ ] **Task 7: End-to-end smoke test — real patch release**
  - Acceptance: run `./scripts/release.sh` for real → release commit contains
    Cargo.toml + Cargo.lock + CHANGELOG.md → tag pushed → CI green end-to-end:
    gate passes, both Trivy gates pass, image with 3 tags + SBOM + provenance
    on GHCR, GitHub Release created with git-cliff body.
  - Verify: watch the Actions run; inspect the GHCR package; check the Release.
  - Files: none (consumes everything above)
  - Dependencies: ALL; requires tools installed locally:
    `cargo install git-cliff --locked --version 2.14.1` and
    `cargo install cargo-release --locked --version 1.1.5`

### Checkpoint: Complete
- [ ] All spec success criteria met
- [ ] Release v1.0.3 (or next) published end-to-end

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| cargo-release 1.1.5 local behavior differs from expectation (first real use) | High | Task 2's `--dry-run` exercise; Task 7 is the real gate |
| Trivy DB fetch flakiness in CI | Med | Non-blocking on DB errors (`--skip-db-update` fallback NOT used; accept rerun) |
| `pre-release-hook` pollutes CHANGELOG.md during Task 2 verification | Low | Revert file with `git checkout` after each dry run |
| No docker/podman on this machine for Task 4 | Med | Note as not-testable locally; real build happens in Task 7's CI |
| Multi-arch Trivy scan quirks (manifest lists) | Low | Scan by digest/tag after push; if the scan can't resolve, fall back to scanning amd64 locally built in CI |

## Open questions

None — all resolved in the grilling session; recorded in the spec.