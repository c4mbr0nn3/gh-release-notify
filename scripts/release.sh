#!/usr/bin/env bash
#
# scripts/release.sh — local-driven release workflow.
#
# Determines the next version from conventional commits (git-cliff), runs the
# full verification gate, then executes cargo-release to bump Cargo.toml +
# Cargo.lock, generate the changelog section (pre-release hook), commit
# `chore: release v{version}`, tag `v{version}`, and push branch + tag.
#
# Usage: ./scripts/release.sh [--dry-run] [--major|--minor|--patch]
#
#   --dry-run            print the prospective version, level, changelog
#                        section, and planned actions; write nothing
#   --major|--minor|--patch  force the bump level instead of inference
#
# FIX-FORWARD POLICY:
#   If the script fails AFTER the push has happened, do NOT delete tags or
#   revert. Fix forward: land the correction as a new conventional commit on
#   main and cut a new patch release with this script. Tag deletion and
#   revert dances are forbidden.
#
set -euo pipefail

DRY_RUN=0
BUMP_LEVEL=""
NEXT_VERSION=""
LAST_TAG=""

readonly REQUIRED_GIT_CLIFF="2.14.1"
readonly REQUIRED_CARGO_RELEASE="1.1.5"
readonly PIN_GIT_CLIFF="cargo install git-cliff --version 2.14.1 --locked"
readonly PIN_CARGO_RELEASE="cargo install cargo-release --version 1.1.5 --locked"

usage() {
    grep '^# ' "${BASH_SOURCE[0]}" | head -n -1 | sed 's/^# \?//'
}

check_deps() {
    local version
    if ! command -v git-cliff >/dev/null 2>&1; then
        echo "error: git-cliff not found; install the pinned version:" >&2
        echo "  ${PIN_GIT_CLIFF}" >&2
        return 1
    fi
    version="$(git-cliff --version | awk '{print $2}')"
    if [[ "${version}" != "${REQUIRED_GIT_CLIFF}" ]]; then
        echo "error: git-cliff ${version} found, ${REQUIRED_GIT_CLIFF} required; install the pinned version:" >&2
        echo "  ${PIN_GIT_CLIFF}" >&2
        return 1
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        echo "error: cargo not found" >&2
        return 1
    fi
    if ! cargo release --version >/dev/null 2>&1; then
        echo "error: cargo-release not found; install the pinned version:" >&2
        echo "  ${PIN_CARGO_RELEASE}" >&2
        return 1
    fi
    version="$(cargo release --version | awk '{print $2}')"
    if [[ "${version}" != "${REQUIRED_CARGO_RELEASE}" ]]; then
        echo "error: cargo-release ${version} found, ${REQUIRED_CARGO_RELEASE} required; install the pinned version:" >&2
        echo "  ${PIN_CARGO_RELEASE}" >&2
        return 1
    fi
}

refuse() {
    echo "refused: $1" >&2
    return 1
}

check_state() {
    [[ -z "$(git status --porcelain)" ]] \
        || refuse "worktree is dirty; commit or stash local changes"
    [[ "$(git rev-parse --abbrev-ref HEAD)" == "main" ]] \
        || refuse "HEAD is not on main (release must run from main)"
    [[ "$(git rev-list --count "@{upstream}"..HEAD 2>/dev/null || echo 1)" -eq 0 ]] \
        || refuse "main has unpushed commits; push to origin/main first"
    [[ "$(git rev-list --count "HEAD..@{upstream}" 2>/dev/null || echo 1)" -eq 0 ]] \
        || refuse "local main is behind origin/main; pull first"
    NEXT_VERSION="$(git cliff --bumped-version ${BUMP_LEVEL:+--bump "${BUMP_LEVEL}"} | tail -n 1)"
    LAST_TAG="$(git describe --tags --abbrev=0 2>/dev/null || true)"
    [[ -n "${LAST_TAG}" ]] || refuse "no previous tag found; cannot infer release range"
    if git rev-parse -q --verify "refs/tags/${NEXT_VERSION}" >/dev/null; then
        refuse "tag ${NEXT_VERSION} already exists locally"
    fi
    if git ls-remote --tags origin "refs/tags/${NEXT_VERSION}" | grep -q .; then
        refuse "tag ${NEXT_VERSION} already exists on origin"
    fi
    [[ "$(git rev-list --count "${LAST_TAG}..HEAD")" -ge 1 ]] \
        || refuse "no commits since ${LAST_TAG}; nothing to release"
}

dry_run() {
    echo "dry-run: version:      ${NEXT_VERSION}"
    echo "dry-run: level:        ${BUMP_LEVEL:-inferred}"
    echo "dry-run: changelog section that will be generated:"
    git cliff --unreleased --bump ${BUMP_LEVEL:+"${BUMP_LEVEL}"} 2>/dev/null
    echo "dry-run: planned actions:"
    echo "  1. run gate: cargo fmt --check, cargo clippy -- -D warnings, cargo test"
    echo "  2. cargo release --config release.toml --execute --no-confirm ${NEXT_VERSION#v}"
    echo "     -> bumps Cargo.toml + Cargo.lock, generates changelog via pre-release hook"
    echo "     -> commits 'chore: release ${NEXT_VERSION}', tags ${NEXT_VERSION}, pushes main + tag"
    echo "dry-run complete; nothing was written"
}

execute_release() {
    echo "running local gate..."
    cargo fmt --check
    cargo clippy -- -D warnings
    cargo test
    echo "executing release..."
    cargo release --config release.toml --execute --no-confirm "${NEXT_VERSION#v}"
    echo "release ${NEXT_VERSION} pushed"
    echo "on any failure after the push: fix forward on a new patch; do not delete tags or revert"
}

main() {
    local args=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --dry-run) DRY_RUN=1 ;;
            --major|--minor|--patch)
                [[ -z "${BUMP_LEVEL}" ]] || { echo "error: only one level flag allowed" >&2; return 1; }
                BUMP_LEVEL="${1#--}"
                ;;
            -h|--help) usage; return 0 ;;
            *) echo "error: unknown flag: $1" >&2; usage >&2; return 1 ;;
        esac
        args+=("$1")
        shift
    done

    check_deps
    check_state
    if [[ "${DRY_RUN}" -eq 1 ]]; then
        dry_run
    else
        execute_release
    fi
}

main "$@"
