# Web UI & auth

The daemon serves an embedded axum UI: one HTML page, vanilla JavaScript, and
one hand-written stylesheet, all compiled into the binary with `include_str!`.
There is no npm, no CSS framework, no JavaScript build step, and no separate
frontend artifact. All UI work follows [UI guidelines](UI-GUIDELINES.md).

The UI listens on `[ui].bind_addr`:`[ui].port`, default `127.0.0.1:8080`.
`UI_BIND_ADDR` and `UI_PORT` override both at startup and are never written back
to the config file.

## Page sections

- **Status** — a tick line with last/next poll times, then the repository table
  (last-seen tag and state). States are *Watching* or *No release yet*; the
  snapshot has no per-repository error state.
- **Settings** — polling, repositories, email recipients, SMTP, and admin access.
- **Read-only values** — state path, effective bind address and port, auth mode,
  and environment-managed token flags.
- **Sign in** — one admin-token field, shown only in token mode when there is no
  valid session.

The settings form uses an explicit save model: the save bar tracks
*All changes saved*, *Unsaved changes*, *Saving…*, *Saved HH:MM*, validation
errors, and save errors. It keeps your edits after a session expiry so you can
sign in again and retry. There is no autosave.

## Reload behavior

Saving from the UI writes the config file and publishes the new `Config` on a
`tokio::sync::watch` channel. The scheduler re-reads it after the in-flight tick
finishes, so **no restart is required**. Hand edits to `config.toml` are not
watched and require a restart.

Status flows the other way on a second watch channel; the UI refreshes it
periodically and after a save.

## Auth modes

Exactly one mode is resolved at startup:

| Mode | How to enable | Behavior |
|---|---|---|
| Token | `ADMIN_TOKEN` or `[ui].admin_token` | Login form; token exchanged for an in-memory session cookie. `Authorization: Bearer` is also accepted for API clients. |
| Proxy | `[ui].trust_proxy_auth = true` | Accepts a non-empty `Remote-User` header from a reverse proxy; no login page. |
| Open | neither | Anyone who can reach the port can read and edit the config. |

- Setting a token and `trust_proxy_auth = true` together is a **startup error**.
- Sessions expire after 12 hours and are stored in memory, so a restart logs
  everyone out.
- The cookie is `HttpOnly; SameSite=Strict`; it becomes `Secure` when the
  request carries `X-Forwarded-Proto: https`.
- Login attempts are rate-limited per client address.
- Generate a token with `openssl rand -hex 32`.

## Reverse proxy

The UI binds `127.0.0.1` by default and is not reachable from other hosts until
you expose it.

- The shipped `docker-compose.yml` puts the container on the
  `gh-release-notify-net` network, maps `127.0.0.1:8080:8080` on the host, and
  sets `UI_BIND_ADDR=0.0.0.0` inside the container (container loopback is not
  reachable from the host).
- Route your proxy to `gh-release-notify:8080` on that network.
- For header auth (Authelia, Pangolin, and similar), set
  `trust_proxy_auth = true` and have the proxy set `Remote-User`. This is
  **spoofable if the UI port is directly reachable**, so allow only the proxy to
  connect.
- Keep the host port mapping on loopback unless you intend LAN exposure.
- TLS is terminated by the proxy; the daemon does not serve TLS itself.

The UI displays the *effective* bind address and port, after environment
overrides. A compose deployment therefore shows `0.0.0.0` while the file keeps
`127.0.0.1`.

## Read-only config

The UI can disable saving when the config is not writable. It probes by creating
a temporary `*.probe.*` file next to the config path, not by opening the config
file itself, so the behavior depends on the mount type:

| Mount | Probe | Result |
|---|---|---|
| Config **directory** `:ro` | fails | Banner shown, saving disabled. Supported hardening option. |
| Config **directory** read-write | succeeds | Saving works. Required for UI edits. |
| Single **file** `:ro` | succeeds (parent is writable) | No banner; a save fails with a plain read-only error. |
| Single **file** read-write | succeeds | No banner; a save fails with a bind-mount rename error (`EBUSY`). |

Mount the **directory**, not the file:

```yaml
volumes:
  - ./config:/config          # UI can save
  # - ./config:/config:ro     # immutable; banner shown, saving disabled
```

A single-file bind mount cannot support atomic tmp+rename writes. The compose
file and its migration notes assume a directory mount.

## Security posture

- The UI can rewrite the file that holds the SMTP password. In a read-write
  deployment, an admin token or proxy auth is effectively mandatory.
- No secret value is returned by any endpoint or written to the logs. Responses
  report presence flags only (`admin_token_set`, `env_managed`). The UI does not
  know whether an SMTP password exists in the config file.
- Binding to `0.0.0.0` with no token logs a warning at startup.
- Every non-GET request, including `POST /login`, `PUT /api/config`, and
  `POST /api/logout`, requires `X-Requested-With: gh-release-notify`. Combined
  with `SameSite=Strict`, this is the CSRF defense.

## Health endpoint

`GET /healthz` is public and returns JSON:

```json
{
  "status": "ok",
  "uptime_seconds": 42,
  "next_poll_at": "2026-10-20T12:16:50Z"
}
```
