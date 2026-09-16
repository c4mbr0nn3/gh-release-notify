#!/usr/bin/env bash
#
# scripts/backfill-release-notes.sh — repair GitHub Release bodies for tags
# whose published notes were rendered empty by the pre-fix release workflow.
#
# Context: before the fix, release.yml ran `git cliff --unreleased` while HEAD
# was already the release tag, so the commit range resolved empty and the
# GitHub Release body contained only the version heading. This script
# regenerates the correct body for already-published tags and PATCHes the
# existing Release.
#
# It NEVER creates a Release: a tag with no published Release is refused, so
# tags that intentionally have no Release (v1.1.0, v1.1.1, v1.1.2) cannot be
# backfilled by accident.
#
# Usage: ./scripts/backfill-release-notes.sh [--dry-run] [--tag <tag>]...
#
#   --dry-run     regenerate bodies and report the Release that would be
#                 patched, using unauthenticated read-only GETs; no token
#                 needed, nothing written
#   --tag <tag>   backfill only this tag (repeatable); default is the list of
#                 tags already published with an empty body
#
# Requires: GITHUB_TOKEN (write mode only), with write access to the repo's
# contents. A classic PAT needs `repo`; a fine-grained PAT needs
# Contents: Read and write on the repository.
#
set -euo pipefail

GITHUB_REPO="c4mbr0nn3/gh-release-notify"
API_ROOT="https://api.github.com/repos/${GITHUB_REPO}"
REQUIRED_GIT_CLIFF="2.14.1"

# Tags published with an empty body. v1.0.x are omitted (their bodies are
# valid GitHub-generated notes); v1.1.0-v1.1.2 are omitted (no Release exists).
DEFAULT_TAGS=(v1.1.3 v1.1.4 v1.1.5)

DRY_RUN=0
TAGS=()
TMP_DIR=""

usage() {
    grep '^# ' "${BASH_SOURCE[0]}" | head -n -2 | sed 's/^# \?//'
}

die() {
    echo "error: $1" >&2
    exit 1
}

cleanup() {
    [[ -n "${TMP_DIR}" ]] && rm -rf "${TMP_DIR}"
    return 0
}

check_deps() {
    local missing=()
    command -v git-cliff >/dev/null 2>&1 || missing+=(git-cliff)
    command -v jq >/dev/null 2>&1 || missing+=(jq)
    command -v curl >/dev/null 2>&1 || missing+=(curl)
    (( ${#missing[@]} )) && die "missing required command(s): ${missing[*]}"
    local version
    version="$(git-cliff --version | awk '{print $2}')"
    if [[ "${version}" != "${REQUIRED_GIT_CLIFF}" ]]; then
        die "git-cliff ${version} found, ${REQUIRED_GIT_CLIFF} required; install the pinned version:
  cargo install git-cliff --version ${REQUIRED_GIT_CLIFF} --locked"
    fi
    if [[ "${DRY_RUN}" -eq 0 && -z "${GITHUB_TOKEN:-}" ]]; then
        die "GITHUB_TOKEN is not set; export a token with contents:write, or use --dry-run"
    fi
}

# Read a Release by tag. Unauthenticated when no token is available (public repo).
release_id_for_tag() {
    local tag="$1" response
    if [[ -n "${GITHUB_TOKEN:-}" ]]; then
        response="$(curl -sS -H "Authorization: Bearer ${GITHUB_TOKEN}" \
            -H "Accept: application/vnd.github+json" \
            -H "X-GitHub-Api-Version: 2022-11-28" \
            "${API_ROOT}/releases/tags/${tag}")"
    else
        response="$(curl -sS -H "Accept: application/vnd.github+json" \
            -H "X-GitHub-Api-Version: 2022-11-28" \
            "${API_ROOT}/releases/tags/${tag}")"
    fi
    printf '%s' "${response}" | jq -r '.id // empty'
}

patch_release() {
    local id="$1" payload="$2" response code
    response="$(curl -sS -X PATCH \
        -H "Authorization: Bearer ${GITHUB_TOKEN}" \
        -H "Accept: application/vnd.github+json" \
        -H "X-GitHub-Api-Version: 2022-11-28" \
        -w '\n%{http_code}' \
        -d "${payload}" \
        "${API_ROOT}/releases/${id}")"
    code="${response##*$'\n'}"
    if [[ "${code}" == "200" ]]; then
        return 0
    fi
    echo "  HTTP ${code}: $(printf '%s' "${response%$'\n'*}" | jq -r '.message // "unknown error"' 2>/dev/null)" >&2
    return 1
}

render_body() {
    local clone="$1" tag="$2" out="$3"
    git -C "${clone}" checkout -q -f --detach "${tag}"
    git cliff \
        --repository "${clone}" \
        --config "${REPO_ROOT}/cliff.toml" \
        --current \
        --strip all > "${out}"
    [[ -s "${out}" ]] || die "git-cliff produced an empty body for ${tag}"
    grep -q '^## ' "${out}" || die "git-cliff body for ${tag} has no version heading"
    grep -q '^- ' "${out}" \
        || die "git-cliff body for ${tag} lists no commits; refusing to overwrite published notes"
}

main() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --dry-run) DRY_RUN=1 ;;
            --tag)
                shift
                [[ -n "${1:-}" ]] || die "--tag requires a value"
                TAGS+=("$1")
                ;;
            -h|--help) usage; exit 0 ;;
            *) die "unknown flag: $1" ;;
        esac
        shift
    done

    REPO_ROOT="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)" \
        || die "not inside a git repository"
    (( ${#TAGS[@]} )) || TAGS=("${DEFAULT_TAGS[@]}")

    check_deps

    TMP_DIR="$(mktemp -d)"
    trap cleanup EXIT

    if [[ "${DRY_RUN}" -eq 1 ]]; then
        echo "backfilling ${#TAGS[@]} release(s) in ${GITHUB_REPO} (dry-run; nothing will be written)"
    else
        echo "backfilling ${#TAGS[@]} release(s) in ${GITHUB_REPO}"
    fi

    git clone -q --no-hardlinks "${REPO_ROOT}" "${TMP_DIR}/repo"
    if [[ "${DRY_RUN}" -eq 0 ]]; then
        git -C "${TMP_DIR}/repo" fetch -q --tags origin \
            || echo "warning: could not fetch tags from origin; using local tags only" >&2
    fi

    local tag body_file body_bytes id payload failures=0
    for tag in "${TAGS[@]}"; do
        echo
        echo "=== ${tag} ==="
        if ! git -C "${TMP_DIR}/repo" rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
            echo "skipped: tag ${tag} not found; run 'git fetch --tags' first" >&2
            failures=$((failures + 1))
            continue
        fi

        body_file="${TMP_DIR}/${tag}.md"
        render_body "${TMP_DIR}/repo" "${tag}" "${body_file}"
        body_bytes="$(wc -c < "${body_file}")"
        echo "--- regenerated body (${body_bytes} bytes) ---"
        cat "${body_file}"
        echo "--- end body ---"

        id="$(release_id_for_tag "${tag}")"
        if [[ -z "${id}" ]]; then
            echo "refused: no published Release for ${tag}; this script never creates releases" >&2
            failures=$((failures + 1))
            continue
        fi

        if [[ "${DRY_RUN}" -eq 1 ]]; then
            echo "dry-run: would PATCH ${API_ROOT}/releases/${id} with the body above"
            continue
        fi

        payload="$(jq -Rs '{body: .}' < "${body_file}")"
        if patch_release "${id}" "${payload}"; then
            echo "updated ${tag} (release id ${id}, ${body_bytes} bytes)"
        else
            echo "failed: could not update ${tag} (release id ${id})" >&2
            failures=$((failures + 1))
        fi
    done

    echo
    if [[ "${failures}" -gt 0 ]]; then
        die "${failures} tag(s) were not backfilled"
    fi
    echo "done${DRY_RUN:+ (dry-run; nothing was written)}"
}

main "$@"
