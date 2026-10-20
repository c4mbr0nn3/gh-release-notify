#!/usr/bin/env bash
set -euo pipefail

IMAGE_NAME="gh-release-notify:test"
CONTAINER_NAME="gh-release-notify-test"

FORCE=false
NO_CACHE=false
KEEP=false

usage() {
  cat <<EOF
Usage: $0 [-f] [-n] [-k] [-h]

Local podman smoke test for the gh-release-notify container image.

  -f|--force      Remove the existing ${IMAGE_NAME} image before building
  -n|--no-cache   Build with podman build --no-cache
  -k|--keep       Leave the container running for inspection on success
                  (skips the SIGTERM check and prints the UI URL)
  -h|--help       Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -f|--force) FORCE=true; shift ;;
    -n|--no-cache) NO_CACHE=true; shift ;;
    -k|--keep) KEEP=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 1 ;;
  esac
done

if ! command -v podman >/dev/null 2>&1; then
  echo "ERROR: podman is not installed or not on PATH." >&2
  echo "This test requires podman (docker-first environments: install podman or adapt the script)." >&2
  exit 1
fi

if ! command -v timeout >/dev/null 2>&1; then
  echo "ERROR: 'timeout' (coreutils) is not installed or not on PATH." >&2
  exit 1
fi

TMPDIR_TEST=""

cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  if podman container exists "${CONTAINER_NAME}" 2>/dev/null; then
    if [[ $rc -ne 0 ]]; then
      echo "--- Dumping logs of failed container: ${CONTAINER_NAME} ---" >&2
      podman logs "${CONTAINER_NAME}" >&2 || true
    fi
    if [[ $rc -ne 0 || $KEEP = false ]]; then
      podman rm -f "${CONTAINER_NAME}" >/dev/null 2>&1 || true
    else
      echo "Keeping container ${CONTAINER_NAME} for inspection (-k)."
    fi
  fi
  if podman container exists "${CONTAINER_NAME}-invalid" 2>/dev/null; then
    podman rm -f "${CONTAINER_NAME}-invalid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$TMPDIR_TEST" && -d "$TMPDIR_TEST" ]]; then
    if [[ $rc -eq 0 && $KEEP = true ]]; then
      echo "Keeping temp config/state at ${TMPDIR_TEST} (-k)."
    else
      rm -rf "$TMPDIR_TEST" || true
    fi
  fi
  exit "$rc"
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT

cd "$(dirname "$0")/.."

echo "============================================================"
echo "Phase 0: build image ${IMAGE_NAME}"
echo "============================================================"

if [[ $FORCE = true ]]; then
  if podman image exists "${IMAGE_NAME}"; then
    DEPENDENTS="$(podman ps -a --filter "ancestor=${IMAGE_NAME}" --format '{{.Names}}' || true)"
    if [[ -n "$DEPENDENTS" ]]; then
      echo "ERROR: cannot --force-remove image ${IMAGE_NAME}: existing containers use it:" >&2
      echo "$DEPENDENTS" >&2
      echo "Remove them first, e.g.: podman rm -f \$(podman ps -aq --filter ancestor=${IMAGE_NAME})" >&2
      exit 1
    fi
    echo "Force: removing existing image ${IMAGE_NAME}"
    podman rmi "${IMAGE_NAME}"
  fi
fi

BUILD_ARGS=(--tag "${IMAGE_NAME}")
if [[ $NO_CACHE = true ]]; then
  BUILD_ARGS+=(--no-cache)
fi

echo "podman build ${BUILD_ARGS[*]} ."
podman build "${BUILD_ARGS[@]}" .
echo "PASS: image built"

echo ""
echo "============================================================"
echo "Phase 1: happy path (boot, poll, persist, SIGTERM)"
echo "============================================================"

TMPDIR_TEST="$(mktemp -d /tmp/gh-release-notify-test.XXXXXX)"
chmod 755 "${TMPDIR_TEST}"

cat > "${TMPDIR_TEST}/config.toml" <<'EOF'
poll_interval_seconds = 60
state_path = "/state/state.json"
sender = "test@example.com"
repos = ["fosrl/pangolin"]
recipients = ["test@example.com"]

[smtp]
host = "127.0.0.1"
port = 25
encryption = "none"
username = ""
password = ""
EOF
chmod 644 "${TMPDIR_TEST}/config.toml"

mkdir -p "${TMPDIR_TEST}/state"
chmod 777 "${TMPDIR_TEST}/state"

if podman container exists "${CONTAINER_NAME}"; then
  echo "NOTICE: pre-existing container ${CONTAINER_NAME} found (e.g. from a previous -k run); removing it."
  podman rm -f "${CONTAINER_NAME}"
fi

echo "Starting container ${CONTAINER_NAME}..."
podman run -d --name "${CONTAINER_NAME}" \
  -p "127.0.0.1::8080" \
  -v "${TMPDIR_TEST}/config.toml:/config/config.toml:ro,Z" \
  -v "${TMPDIR_TEST}/state:/state:rw,Z" \
  -e RUST_LOG=info \
  -e UI_BIND_ADDR=0.0.0.0 \
  "${IMAGE_NAME}"

wait_for_log() {
  local pattern="$1" timeout="$2"
  local elapsed=0
  while [[ $elapsed -lt $timeout ]]; do
    LOGS="$(podman logs "${CONTAINER_NAME}" 2>/dev/null || true)"
    if grep -q "$pattern" <<<"$LOGS"; then
      return 0
    fi
    if ! podman container exists "${CONTAINER_NAME}" 2>/dev/null || \
       [[ "$(podman inspect --format '{{.State.Running}}' "${CONTAINER_NAME}" 2>/dev/null)" != "true" ]]; then
      echo "FAIL: container ${CONTAINER_NAME} is not running while waiting for '${pattern}'" >&2
      echo "--- container logs ---" >&2
      podman logs "${CONTAINER_NAME}" >&2 || true
      return 1
    fi
    sleep 1
    elapsed=$((elapsed + 1))
  done
  echo "FAIL: log line '${pattern}' did not appear within ${timeout}s" >&2
  echo "--- container logs ---" >&2
  podman logs "${CONTAINER_NAME}" >&2 || true
  return 1
}

echo "Waiting up to 45s for 'config loaded:' ..."
wait_for_log "config loaded:" 45
echo "PASS: config loaded"

wait_for_log "polling 1 repos" 10
echo "PASS: scheduler polling"

echo "Waiting up to 30s for state file with key 'fosrl/pangolin' (best-effort; GitHub may be unreachable) ..."
STATE_OK=false
elapsed=0
while [[ $elapsed -lt 30 ]]; do
  if [[ -f "${TMPDIR_TEST}/state/state.json" ]] && grep -q "fosrl/pangolin" "${TMPDIR_TEST}/state/state.json"; then
    STATE_OK=true
    break
  fi
  sleep 1
  elapsed=$((elapsed + 1))
done
if [[ $STATE_OK = true ]]; then
  echo "PASS: state persisted to ${TMPDIR_TEST}/state/state.json"
else
  echo "WARNING: state.json with key fosrl/pangolin never appeared — GitHub may be unreachable from this environment." \
       "The state-file check is best-effort; continuing." >&2
fi

if [[ $KEEP = false ]]; then
  echo "Sending SIGTERM via podman stop --time 35 ..."
  podman stop --time 35 "${CONTAINER_NAME}" >/dev/null

  EXIT_CODE="$(podman inspect --format '{{.State.ExitCode}}' "${CONTAINER_NAME}")"
  if [[ "$EXIT_CODE" != "0" ]]; then
    echo "FAIL: container exit code after SIGTERM was ${EXIT_CODE}, expected 0" >&2
    podman logs "${CONTAINER_NAME}" >&2 || true
    exit 1
  fi
  echo "PASS: clean exit code 0"

  LOGS="$(podman logs "${CONTAINER_NAME}")"
  if ! grep -q "received SIGTERM" <<<"$LOGS"; then
    echo "FAIL: logs did not contain 'received SIGTERM'" >&2
    exit 1
  fi
  if ! grep -q "shutdown complete" <<<"$LOGS"; then
    echo "FAIL: logs did not contain 'shutdown complete'" >&2
    exit 1
  fi
  echo "PASS: graceful shutdown logged"

  podman rm "${CONTAINER_NAME}" >/dev/null
fi

echo ""
echo "============================================================"
echo "Phase 2: invalid config rejected"
echo "============================================================"

cat > "${TMPDIR_TEST}/config-invalid.toml" <<'EOF'
poll_interval_seconds = 30
state_path = "/state/state.json"
sender = "test@example.com"
repos = ["fosrl/pangolin"]
recipients = ["test@example.com"]

[smtp]
host = "127.0.0.1"
port = 25
encryption = "none"
username = ""
password = ""
EOF
chmod 644 "${TMPDIR_TEST}/config-invalid.toml"

set +e
INVALID_LOGS="$(timeout 30 podman run --rm --name "${CONTAINER_NAME}-invalid" \
  -v "${TMPDIR_TEST}/config-invalid.toml:/config/config.toml:ro,Z" \
  -e RUST_LOG=info \
  "${IMAGE_NAME}" 2>&1)"
INVALID_RC=$?
set -e

if [[ $INVALID_RC -eq 124 ]]; then
  echo "FAIL: invalid-config run timed out after 30s" >&2
  echo "--- output ---" >&2
  echo "$INVALID_LOGS" >&2
  exit 1
fi
if [[ $INVALID_RC -ne 1 ]]; then
  echo "FAIL: invalid-config run exited with ${INVALID_RC}, expected 1" >&2
  echo "--- output ---" >&2
  echo "$INVALID_LOGS" >&2
  exit 1
fi
if ! grep -q "invalid config: poll_interval_seconds must be >= 60" <<<"$INVALID_LOGS"; then
  echo "FAIL: invalid-config run did not log expected validation error" >&2
  echo "--- output ---" >&2
  echo "$INVALID_LOGS" >&2
  exit 1
fi
echo "PASS: invalid config rejected with exit code 1 and validation error"

if [[ $KEEP = true ]]; then
  UI_PORT="$(podman port "${CONTAINER_NAME}" 8080/tcp 2>/dev/null | sed -n 's/.*:\([0-9]\+\)$/\1/p' | head -1)"
  echo ""
  echo "Container ${CONTAINER_NAME} left running (-k)."
  if podman logs "${CONTAINER_NAME}" 2>/dev/null | grep -q "ui listening on"; then
    echo "  UI:     http://127.0.0.1:${UI_PORT:-8080}"
  fi
  echo "  Config: ${TMPDIR_TEST}/config.toml"
  echo "  State:  ${TMPDIR_TEST}/state/state.json"
  echo "  Logs:   podman logs -f ${CONTAINER_NAME}"
  echo "  Stop:   podman stop ${CONTAINER_NAME} && podman rm ${CONTAINER_NAME}"
fi

echo ""
echo "============================================================"
echo "PASS: all phases succeeded"
echo "  - image ${IMAGE_NAME} built"
echo "  - daemon booted, config loaded, scheduler polling"
echo "  - state persisted (best-effort check: $([[ $STATE_OK = true ]] && echo ok || echo skipped/unreachable))"
if [[ $KEEP = true ]]; then
  echo "  - container left running for inspection (SIGTERM check skipped)"
else
  echo "  - SIGTERM -> exit code 0 with 'received SIGTERM' and 'shutdown complete'"
fi
echo "  - invalid config rejected (exit 1, validation error logged)"
if [[ $KEEP = true && -n "${UI_PORT:-}" ]]; then
  echo "  - UI reachable at http://127.0.0.1:${UI_PORT} (if this build has the web UI)"
fi
if [[ $KEEP = true ]]; then
  echo "  - test container kept: run 'podman logs ${CONTAINER_NAME}' to inspect"
fi
echo "============================================================"