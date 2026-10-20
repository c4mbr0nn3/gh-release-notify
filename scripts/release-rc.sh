#!/usr/bin/env bash
#
# scripts/release-rc.sh — tag-only release-candidate workflow.
#
# Creates and pushes an RC tag vX.Y.Z-rc.N (N starts at 0). RC tags are
# TAG-ONLY: this script never commits, never modifies Cargo.toml/Cargo.lock,
# and never writes a version file. RC tags never render a CHANGELOG.md
# section, never create a GitHub Release, and never publish the :latest
# image tag — CI publishes :<ver>-rc.<n> plus a moving :rc tag instead.
#
# Usage: ./scripts/release-rc.sh [--base X.Y.Z] [-d|--dry-run] [-y|--yes] [-h|--help]
#
#   --base X.Y.Z   base version for the RC (default: next stable version
#                  derived from `git cliff --bumped-version`)
#   -d, --dry-run  preview the tag without creating or pushing it
#   -y, --yes      skip the confirmation prompt
#
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

readonly REQUIRED_GIT_CLIFF="2.14.1"
readonly PIN_GIT_CLIFF="cargo install git-cliff --version 2.14.1 --locked"

DRY_RUN=false
AUTO_CONFIRM=false
BASE=""

print_usage() {
    cat << EOF
Usage: $0 [OPTIONS]

Create and push a release-candidate tag vX.Y.Z-rc.N (tag-only, no commits).

Options:
  --base X.Y.Z    Base version for the RC (default: next stable version)
  -d, --dry-run   Preview the tag without creating or pushing it
  -y, --yes       Skip the confirmation prompt
  -h, --help      Display this help message

Examples:
  $0                    # RC for the next derived stable version
  $0 --base 1.2.3       # RC for 1.2.3
  $0 --base 1.2.3 -d    # Preview the next 1.2.3 RC tag
EOF
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --base)
            if [[ -z "${2:-}" ]]; then
                echo -e "${RED}Error: --base requires a version argument${NC}"
                exit 1
            fi
            BASE="$2"
            shift 2
            ;;
        -d|--dry-run)
            DRY_RUN=true
            shift
            ;;
        -y|--yes)
            AUTO_CONFIRM=true
            shift
            ;;
        -h|--help)
            print_usage
            ;;
        *)
            echo -e "${RED}Error: Unknown argument '$1'${NC}"
            echo "Use -h or --help for usage information"
            exit 1
            ;;
    esac
done

# The base version derives from git-cliff, so the pinned version is required.
if ! command -v git-cliff >/dev/null 2>&1; then
    echo -e "${RED}Error: git-cliff not found; install the pinned version:${NC}"
    echo "  ${PIN_GIT_CLIFF}"
    exit 1
fi
installed_ver="$(git-cliff --version | awk '{print $2}')"
if [[ "${installed_ver}" != "${REQUIRED_GIT_CLIFF}" ]]; then
    echo -e "${RED}Error: git-cliff ${installed_ver} found, ${REQUIRED_GIT_CLIFF} required; install the pinned version:${NC}"
    echo "  ${PIN_GIT_CLIFF}"
    exit 1
fi

# Resolve the base version, defaulting to the next stable version.
# tail -1 keeps the last token in case git-cliff prints extra output.
if [[ -z "$BASE" ]]; then
    BASE=$(git cliff --bumped-version 2>/dev/null | tail -1 | grep -oE 'v?[0-9]+\.[0-9]+\.[0-9]+' | sed 's/^v//' || true)
fi

if [[ ! "$BASE" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo -e "${RED}Error: could not determine a valid base version (got '$BASE')${NC}"
    echo "Pass one explicitly, e.g.: $0 --base 1.2.3"
    exit 1
fi

# Determine the next free RC number for this base from BOTH local and remote
# tags. A stale local clone must not reuse a number already published to
# origin (which would silently skip an RC number). `--refs` omits peeled
# `^{}` entries so the numeric suffix parses cleanly.
# Fail closed: without fresh remote tags the RC number could be numbered
# from a stale local clone and silently skip an RC number already on origin.
if ! git fetch --tags origin >/dev/null 2>&1; then
    echo -e "${RED}Error: 'git fetch --tags origin' failed; cannot verify remote RC tags.${NC}"
    echo "Fix network/auth for origin and retry."
    exit 1
fi
LATEST_LOCAL=$(git tag --list "v${BASE}-rc.*" --sort=-v:refname | head -1)
LATEST_REMOTE=$(git ls-remote --tags --refs origin "v${BASE}-rc.*" 2>/dev/null \
    | sed -n 's#.*refs/tags/##p' | sort -V | tail -1) \
    || { echo -e "${RED}Error: 'git ls-remote origin' failed; cannot verify remote RC tags.${NC}"; exit 1; }
LATEST_RC=""
for candidate in "$LATEST_LOCAL" "$LATEST_REMOTE"; do
    [[ -z "$candidate" ]] && continue
    if [[ -z "$LATEST_RC" ]] || (( ${candidate##*-rc.} > ${LATEST_RC##*-rc.} )); then
        LATEST_RC="$candidate"
    fi
done
if [[ -z "$LATEST_RC" ]]; then
    RC_NUMBER=0
else
    RC_NUMBER=$(( ${LATEST_RC##*-rc.} + 1 ))
fi
TAG="v${BASE}-rc.${RC_NUMBER}"

# Refuse to tag a dirty working tree (the tag would point at a commit that
# does not include the uncommitted changes, which is confusing on an RC).
if [[ -n $(git status --porcelain | grep -v "^?? " || true) ]]; then
    echo -e "${RED}Error: You have uncommitted changes. Commit or stash them first.${NC}"
    git status --short
    exit 1
fi

if [[ "$(git rev-parse --abbrev-ref HEAD)" != "main" ]]; then
    echo -e "${RED}Error: HEAD is not on main (RC tags must be cut from main).${NC}"
    exit 1
fi

# Upstream sync guards, matching scripts/release.sh.
[[ "$(git rev-list --count "@{upstream}"..HEAD 2>/dev/null || echo 1)" -eq 0 ]] \
    || { echo -e "${RED}Error: main has unpushed commits; push to origin/main before tagging an RC.${NC}"; exit 1; }
[[ "$(git rev-list --count "HEAD..@{upstream}" 2>/dev/null || echo 1)" -eq 0 ]] \
    || { echo -e "${RED}Error: local main is behind origin/main; pull first.${NC}"; exit 1; }

# Collision checks (local and remote).
if git rev-parse "$TAG" >/dev/null 2>&1; then
    echo -e "${RED}Error: Tag $TAG already exists locally${NC}"
    exit 1
fi
REMOTE_TAGS="$(git ls-remote --tags --refs origin 2>/dev/null)" \
    || { echo -e "${RED}Error: 'git ls-remote origin' failed; cannot verify remote tags.${NC}"; exit 1; }
if echo "$REMOTE_TAGS" | grep -q "refs/tags/$TAG$"; then
    echo -e "${RED}Error: Tag $TAG already exists on remote${NC}"
    exit 1
fi

if [[ "$DRY_RUN" == true ]]; then
    echo -e "${YELLOW}[DRY-RUN MODE]${NC} All guards evaluated; no tag would be created or pushed."
    echo -e "${YELLOW}Plan:${NC} tag-only: push $TAG (annotated, no commit, no"
    echo "Cargo.toml/Cargo.lock change, no version file) — no CHANGELOG.md"
    echo "section, no GitHub Release; CI publishes :$BASE-rc.$RC_NUMBER and"
    echo "the moving :rc tag; never :latest."
    exit 0
fi

echo -e "${YELLOW}Release candidate:${NC}"
echo "  Base:   $BASE"
echo "  RC #:   $RC_NUMBER"
echo "  Tag:    $TAG"
echo "  HEAD:   $(git rev-parse --short HEAD) $(git log -1 --pretty=%s)"
echo ""
echo "This creates an annotated tag and pushes it to origin. TAG-ONLY:"
echo "  - no commit, no Cargo.toml/Cargo.lock changes, no version file"
echo "  - no CHANGELOG.md section (RC tags never appear in the changelog)"
echo "  - no GitHub Release (release page) is created"
echo "  - CI publishes :$BASE-rc.$RC_NUMBER and the moving :rc tag; never :latest"
echo ""

if [[ "$AUTO_CONFIRM" != true ]]; then
    read -p "Continue? [y/N] " -n 1 -r
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo -e "${YELLOW}Aborted by user${NC}"
        exit 0
    fi
fi

git tag -a "$TAG" -m "Release candidate $TAG"
echo -e "${GREEN}✓${NC} Created tag $TAG"

git push origin "$TAG"
echo -e "${GREEN}✓${NC} Pushed tag $TAG to remote"

echo ""
echo -e "${GREEN}🎉 Release candidate tagged!${NC}"
echo -e "Version: ${GREEN}${BASE}-rc.${RC_NUMBER}${NC}"
echo ""
echo "Once the RC is validated, cut the stable release with: ./scripts/release.sh"