# Operations

## First-run behavior

For each repo with no stored tag, the first successful poll records the current
latest tag **without** sending an email. Later releases trigger notifications.
Delete the state file to reset this behavior.

## Logs

Structured logs use `tracing`. The default filter is `info`; set `RUST_LOG` for
more detail (`debug`, `gh_release_notify=debug`).

Normal tick:

```text
polling 2 repos
no change for fosrl/pangolin (still 1.24.0)
no change for fosrl/newt (still 1.18.1)
tick complete, next poll at 2026-10-20 13:00:00 UTC
```

New release:

```text
new release detected for fosrl/pangolin: 1.25.0
sent notification to 2 recipients for fosrl/pangolin 1.25.0
state saved to /state/state.json
```

Rate limited:

```text
github rate limit low: 9 requests remaining
github rate-limited, skipping remaining repos this tick: github rate-limited (403) for fosrl/pangolin: ...
rate-limited by github, skipping remaining repos this tick
```

The daemon skips the rest of the current tick after a 403. The next tick retries
the remaining repos.

## State

- Format: a JSON object mapping `owner/repo` to the last-seen tag.
- A missing file starts empty. A corrupt file logs a warning and starts empty.
- Saves are atomic: write `<path>.tmp`, then rename over the target.
- On SMTP failure the tag is not advanced, so the next tick retries the email.
- On the first run the tag is stored even if the state save fails; the save error
  is logged.

## Permissions

The container runs as the non-root user `ghrel`, uid **10001**. Both mounted
directories must be writable by that uid:

```bash
mkdir -p config state
sudo chown -R 10001:10001 config state
```

`config` must be writable for UI saves; `state` must be writable for state saves.
Run the `chown` after you finish editing the files, because the directories
become container-owned; later host-side edits need `sudo` or matching group
access. A read-only config directory is supported, but the UI then disables
saving (see [Web UI & auth](web-ui.md#read-only-config)).

## Health

`GET /healthz` needs no auth and returns:

```json
{
  "status": "ok",
  "uptime_seconds": 3600,
  "next_poll_at": "2026-10-20T13:00:00Z"
}
```

## Upgrade and rollback

```bash
docker compose pull
docker compose up -d
```

The state volume survives container recreation, so already-seen releases do not
produce a notification burst. To roll back, pin the previous image tag in
`docker-compose.yml` and run the same commands.

## Building the image

```bash
docker build -t gh-release-notify:latest .
# or, when docker is unavailable:
podman build -t gh-release-notify:latest .
```

## Migrating from a single-file config mount

Older deployments mounted `./config.toml` directly. The UI saves with tmp+rename
and cannot replace a bind-mounted file, so move to a directory mount before
updating the compose file:

```bash
mkdir -p config
mv config.toml config/config.toml
sudo chown -R 10001:10001 config
```

The container user needs write access to the directory, exactly like `state`.

## Container smoke test

`./scripts/test-container.sh` builds `gh-release-notify:test`, boots it against a
temporary config and state directory, and checks config loading, polling,
graceful SIGTERM shutdown, and invalid-config rejection. State persistence is
checked best-effort, so the script passes when GitHub is unreachable.

The script requires **podman** and exits if it is missing. Flags are listed in
`--help`; notable ones:

- `-f|--force` — remove the existing test image first.
- `-n|--no-cache` — build without the cache.
- `-k|--keep` — leave the container running, skip the SIGTERM check, and print
  the UI URL.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `failed to write state tmp file ... Permission denied` | `./state` is not writable by uid 10001 | `sudo chown -R 10001:10001 state` |
| `No such file or directory` when saving state | The `state_path` parent directory does not exist | Create the directory or override `--state-path` |
| `failed to rename ... bind-mounted file` | A single-file read-write mount cannot be replaced atomically | Mount the config directory, not the file |
| Save fails with a read-only error, but no banner is shown | A single-file `:ro` mount lets the parent-directory probe succeed | Mount the directory `:ro` to get the banner, or mount it read-write to save |
| `github rate-limited (403)` | Unauthenticated limit (60 requests/hour) | Set `GITHUB_TOKEN` (5,000 requests/hour) |
| UI unreachable from another host | Default bind is `127.0.0.1` | Set `UI_BIND_ADDR`, publish the port, or use a reverse proxy |
| No email on first start | Expected first-run behavior | Delete the state file only if you want a fresh baseline |
| UI edits seem delayed | The scheduler finishes its current tick before applying the new config | Wait for the tick to end; no restart is needed |
