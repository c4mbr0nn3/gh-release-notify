# WEB-UI — Design Spec

**Date:** 2026-09-14
**Status:** Approved
**Module id:** `web-ui`
**Supersedes:** None (extends `2026-07-04-gh-release-notify-design.md`)
**Depends on:** `config-writeback` (calls `config_writeback::apply`)

## Summary

Serve a lightweight, modern-looking web UI directly from the daemon, so a
single operator can view status and edit configuration without hand-editing
`config.toml` and restarting the container. The UI is a single self-contained
HTML file with vanilla JS and vendored Pico CSS, embedded in the binary with
`include_str!`. There is no build step, no npm, and no separate frontend
artifact.

Access is controlled by exactly one of three modes: an admin token (login
page), a trusted reverse-proxy header (no login page), or neither (open, with
exposure delegated entirely to the deployer's own auth layer). Configuring both
a token and proxy trust at the same time is a **startup error** — the modes are
mutually exclusive by design, so there is never a question of which gate is
authoritative.

## Motivation

Editing this daemon's configuration currently means: shell into the host, edit
`config.toml`, restart the container. That is a poor fit for the two most
common operations — "change the poll schedule" and "add a repo" — both of which
are single-value edits. A status view also answers the operator's two real
questions ("is it alive?" and "when does it next poll?") which today require
reading logs.

The constraint that shapes every choice below is **few dependencies, no build
toolchain, no JS framework**. This is a homelab daemon with a 162-package tree;
the UI must not double it or introduce a node build stage.

## Decisions (from design grilling)

| Decision | Choice |
|---|---|
| HTTP framework | `axum` 0.8.9 — measured at +5 packages over the current tree, because `reqwest` already pulls hyper/http/bytes |
| Frontend | Single `index.html`, vanilla JS, no framework, no build |
| CSS | Pico CSS 2.1.1 `pico.classless.min.css` (+71,040 raw bytes; ~10 kB gzipped on the wire), MIT header retained |
| Embodiment | `include_str!` for all three assets |
| Auth modes | Mutually exclusive: token / proxy trust / open. Token + proxy = startup error |
| Session mechanism | In-memory `HashMap<session_id, expiry>`, random 32-byte id, `HttpOnly; SameSite=Strict` cookie, `Secure` when HTTPS detected |
| Token comparison | `subtle::ConstantTimeEq` |
| CSRF | `SameSite=Strict` + required custom header on mutations (no CSRF token) |
| Rate limiting | In-memory per-IP counter on the login endpoint (5/min, reset on success) |
| Reload | `watch<Arc<Config>>`; scheduler re-reads each tick; change notification re-arms the cron sleep |
| Status | `watch<Arc<StatusSnapshot>>` published per tick; `StateStore` ownership unchanged |
| Middleware | One `from_fn_with_state` layer over the whole router with a named public-route allowlist |
| Bind default | `127.0.0.1:8080` in code; `0.0.0.0` only via the shipped compose file |
| Audit logging | `warn!` on auth failure (with IP), `info!` on mutation (old→new for non-secrets, status word for secrets) |
| UI always on | No `enabled` flag (grilling decision) |

## Configuration

New `[ui]` section on `Config`:

```toml
[ui]
bind_addr = "127.0.0.1"
port = 8080
admin_token = ""
trust_proxy_auth = false
```

| Field | Type | Default | UI-editable | Env override |
|---|---|---|---|---|
| `bind_addr` | `String` | `"127.0.0.1"` | No (read-only, "restart required") | No |
| `port` | `u16` | `8080` | No (read-only, "restart required") | No |
| `admin_token` | `String` | `""` (empty = open) | Yes, write-only (empty clears) | `ADMIN_TOKEN` |
| `trust_proxy_auth` | `bool` | `false` | No | No |

`ADMIN_TOKEN` follows the existing lazy-env-override pattern established by
`smtp_password()` (`src/config.rs:48-50`) and `github_token()`
(`src/config.rs:52-57`): the accessor resolves the env var first, falling back
to the config value.

### Validation (added to `Config::validate`, `src/config.rs:59`)

1. **Mutual exclusion:** if the resolved admin token is non-empty **and**
   `trust_proxy_auth` is true, `bail!("ui.admin_token and ui.trust_proxy_auth are mutually exclusive; configure one auth mode")`. This is a hard startup failure, per the grilling decision. A daemon that refuses to start is strictly better than one silently running with an ambiguous security posture.
2. **Port:** reject `port == 0`.
3. **Bind address:** parsed with `std::net::IpAddr::from_str`; a hostname is rejected. This keeps bind semantics unambiguous and avoids a DNS dependency at startup.

## Architecture

### Startup wiring (`src/main.rs`)

```
Config::load ──► Arc<Config>
                      │
                      ├──► watch::channel(Arc<Config>)          config_rx  ──► scheduler
                      │
                      ├──► watch::channel(Arc<StatusSnapshot>)  status_tx  ◄── scheduler
                      │                                          status_rx ──► ui handlers
                      │
                      └──► AppState { config_rx, status_rx, config_path, sessions, login_limiter }
                                     │
                                     ▼
                              tokio::spawn(ui::serve(state, shutdown_rx))
```

The shutdown watch (`watch<bool>`, `src/main.rs:90`) becomes a third receiver:
the axum server must also shut down gracefully, not just the scheduler. The
`select!` at `src/main.rs:99-102` is unchanged.

### Scheduler changes (`src/scheduler.rs`)

`run()` currently takes `cfg: Config` by value and computes the interval once
(`src/scheduler.rs:20`). New signature:

```rust
pub async fn run(
    config_rx: watch::Receiver<Arc<Config>>,
    github: GithubClient,
    mut state: StateStore,
    status_tx: watch::Sender<Arc<StatusSnapshot>>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()>
```

Behavior changes, kept as small as possible:

1. **Per tick:** `let cfg = config_rx.borrow().clone();` at the top of the loop, instead of using a captured-by-value config. The poll body (`src/scheduler.rs:22-66`) is otherwise unchanged — it reads `cfg.repos`, `cfg.recipients`.
2. **Sleep re-arms on config change:** the `select!` that waits (`src/scheduler.rs:67-105`) gains a fourth branch on `config_rx.changed()`. When the operator edits the cron expression or interval, the loop wakes immediately and recomputes its wait instead of finishing the old sleep. This is the concrete payoff of choosing `watch` over `RwLock`.
3. **Status publication:** after each tick, publish `Arc<StatusSnapshot>` via `status_tx.send_replace(...)` (non-failing; see note below).
4. **Mailer construction:** built per send from the current config rather than snapshotted at startup, so edited SMTP settings take effect. `Mailer::new` (`src/notify.rs:35`) is already cheap and infallible-at-build for a fixed host.
5. **GithubClient token:** the client is constructed once (`src/main.rs:66`). A changed `GITHUB_TOKEN` env var cannot be picked up at runtime (Rust env is process-global and the var is an override, not a config field); this is documented, not solved.

**Note on `send_replace`:** `watch::Sender::send` fails when there are no
receivers. `send_replace` cannot fail and is the correct call for a status
publisher. This is not an error to swallow.

### Status snapshot

```rust
pub struct StatusSnapshot {
    pub repos: Vec<RepoStatus>,          // repo + last_seen tag, from StateStore
    pub last_poll_finished_at: Option<DateTime<Utc>>,
    pub next_poll_at: Option<DateTime<Utc>>,
}

pub struct RepoStatus {
    pub repo: String,
    pub last_seen: Option<String>,
}
```

Built by the scheduler after each tick from `state.last_seen(repo)` and the
computed next-occurrence time. `StateStore` keeps its current ownership —
owned by value inside `run()` (`src/scheduler.rs:16`) — and `src/state.rs` is
**not modified at all**. The UI only ever reads the published snapshot, so no
lock that can mutate state is ever handed to an HTTP handler.

`next_poll_at` is known after the wait is computed; publish the snapshot after
computing the next occurrence but before sleeping.

### Router and middleware

```
Router::new()
    // public
    .route("/",               get(serve_index))
    .route("/app.js",         get(serve_js))
    .route("/pico.css",       get(serve_css))
    .route("/login",          post(login))
    .route("/healthz",        get(healthz))
    // protected (auth middleware applies to everything not in PUBLIC_ROUTES)
    .route("/api/config",     get(get_config).put(put_config))
    .route("/api/logout",     post(logout))
    .layer(middleware::from_fn_with_state(state.clone(), auth::require_auth))
    .with_state(state)
```

`PUBLIC_ROUTES` is a **named `const` array** so that adding a public endpoint is
a visible diff in review:

```rust
const PUBLIC_ROUTES: &[&str] = &["/", "/app.js", "/pico.css", "/login", "/healthz"];
```

The middleware is applied to the whole router (not a nested `/api` router) so
that a future endpoint added outside `/api` is protected by default. The
allowlist is an explicit opt-out.

### Auth middleware (`src/ui/auth.rs`)

Evaluation order per request:

1. If the path is in `PUBLIC_ROUTES` → allow.
2. **Proxy mode** (`trust_proxy_auth` true): if `Remote-User` is present and
   non-empty → allow, log at debug with the value. No CIDR check (grilling
   decision: simplicity over completeness; documented as spoofable if the port
   is directly reachable).
3. **Token mode** (resolved token non-empty):
   - Valid session cookie → allow.
   - Valid `Authorization: Bearer <token>` → allow (for `curl`/scripting).
   - Else → 401.
4. **Open mode** (token empty, proxy trust false) → allow.

Mutations (`PUT /api/config`, `POST /api/logout`) additionally require the
custom header `X-Requested-With: gh-release-notify`. With `SameSite=Strict`
this makes a CSRF token unnecessary: HTML forms cannot set the header, and a
cross-origin `fetch` setting a non-simple header triggers a CORS preflight the
server does not answer.

Token comparison uses `subtle::ConstantTimeEq`:

```rust
if presented.as_bytes().ct_eq(expected.as_bytes()).into() { ... }
```

### Session store

```rust
pub struct Sessions {
    map: Mutex<HashMap<String, DateTime<Utc>>>,  // session id -> expiry
}
```

- Session id: 32 random bytes, hex-encoded, from `getrandom::fill(&mut [u8])`
  (`getrandom` 0.4.3, already in `Cargo.lock` via `tempfile`; declaring it
  directly adds **zero** new packages). `rand` is explicitly **not** used — it
  is a heavier graph and unnecessary for a single `fill` call.
- Stored in memory only → a restart logs the operator out. Correct for this
  tool.
- Expiry: 12 hours. Expired entries are evicted lazily on lookup.
- `POST /api/logout` removes the entry and expires the cookie.
- Cookie: `HttpOnly; SameSite=Strict; Path=/`, plus `Secure` when the request
  arrives with `X-Forwarded-Proto: https`.

### Endpoints

| Method | Path | Auth | Purpose |
|---|---|---|---|
| `GET` | `/` | public | Embedded `index.html` |
| `GET` | `/app.js` | public | Embedded `app.js` |
| `GET` | `/pico.css` | public | Embedded Pico CSS |
| `POST` | `/login` | public (rate-limited) | Exchange `admin_token` for a session cookie |
| `GET` | `/healthz` | public | `{"status":"ok","uptime_seconds":N,"next_poll_at":"..."}` |
| `GET` | `/api/config` | protected | Non-secret config + status + `*_set` flags |
| `PUT` | `/api/config` | protected + custom header | Apply `ConfigEdit`, validate, write, publish |
| `POST` | `/api/logout` | protected + custom header | Destroy session |

`PUT` (not `POST`/`PATCH`) because the resource is replaced wholesale and the
operation is idempotent.

### `GET /api/config` response shape

```json
{
  "config": {
    "poll_interval_seconds": 3600,
    "cron_expression": "0 */6 * * *",
    "sender": "bot@homelab.local",
    "repos": ["fosrl/pangolin"],
    "recipients": ["you@example.com"],
    "smtp": { "host": "...", "port": 587, "encryption": "starttls", "username": "..." },
    "ui": { "bind_addr": "127.0.0.1", "port": 8080, "admin_token_set": true, "trust_proxy_auth": false }
  },
  "readonly": { "state_path": "/state/state.json" },
  "env_managed": { "smtp_password": true, "github_token": false },
  "status": {
    "repos": [{ "repo": "fosrl/pangolin", "last_seen": "1.19.4" }],
    "last_poll_finished_at": "2026-09-14T12:00:00Z",
    "next_poll_at": "2026-09-14T18:00:00Z"
  },
  "config_writable": true,
  "auth_mode": "token"
}
```

**Invariant (tested):** no secret value appears anywhere in this payload, in any
encoding. `smtp.password` is absent entirely; `admin_token` is represented only
as the boolean `admin_token_set`. `env_managed` tells the UI to render a field
as disabled with a "managed by environment" label.

`config_writable: false` is what drives the read-only banner and disabling of
save controls (grilling decision: keep `:ro` a supported hardening mode).

### Frontend

Files under `src/ui/`, embedded via `src/ui/assets.rs`:

```rust
pub const INDEX_HTML: &str = include_str!("index.html");
pub const APP_JS:     &str = include_str!("app.js");
pub const PICO_CSS:   &str = include_str!("pico.classless.min.css");
```

`index.html` structure (semantic HTML, classless CSS does the styling):

- A login section (token field + submit), shown only in token mode and when no
  valid session exists.
- A status table: repo / last-seen tag, plus "last poll" and "next poll" lines.
- A settings form: interval (number), cron (text), sender (text), recipients
  (dynamic rows), repos (dynamic rows with add/remove buttons), SMTP host /
  port / encryption (`<select>` of `starttls|tls|none`) / username, and a
  write-only password field with placeholder "unchanged".
- Read-only fields (`state_path`, `bind_addr`, `port`) shown disabled with a
  "restart required" note.
- A read-only banner when `config_writable` is false.

`app.js` responsibilities: fetch config on load, render the form, handle
add/remove rows, `PUT` on submit with the `X-Requested-With` header, display
`{"error": "..."}` payloads inline, and log out.

The CSS file must retain Pico's MIT copyright header, and the README gains a
third-party notice line.

## Error handling

| Case | Response |
|---|---|
| Missing/invalid credentials | `401 {"error":"unauthorized"}` |
| Login attempt over rate limit | `429 {"error":"too many attempts"}` |
| Config validation failure | `400 {"error":"<validator message>"}` |
| Config file not writable | `409 {"error":"config file is not writable: ..."}` |
| Missing `X-Requested-With` on mutation | `400 {"error":"missing X-Requested-With header"}` |
| Malformed request body | `422` (axum rejection) |
| Internal I/O failure | `500 {"error":"..."}` |

Startup failures (exit, matching `src/main.rs:32-38`):
- Token + proxy trust both configured.
- `ui.port == 0` or unparseable `bind_addr`.
- Port already in use → log and exit.

## Security posture (documented in README)

- Default bind is `127.0.0.1`. The shipped `docker-compose.yml` sets `0.0.0.0`
  because container-loopback is unreachable from the host; that file is where
  the exposure decision is visible in `git diff`.
- `0.0.0.0` with no token logs a `WARN` at startup.
- Untrusted `Remote-User` headers are only honored in proxy mode, and proxy
  mode is spoofable if the port is directly reachable. Documented explicitly.
- The UI can rewrite the file that holds the SMTP password, so in a
  read-write container deployment, token or proxy auth is effectively
  mandatory.
- No secret is ever returned by any endpoint or written to logs.

## Testing

In `src/ui/mod.rs` under `#[cfg(test)]`, using `tower::ServiceExt::oneshot`
(no port binding; `tower` arrives with axum):

1. `unauthenticated_request_is_rejected` — token mode, no cookie, `GET
   /api/config` → 401.
2. `wrong_token_is_rejected` — token mode, wrong bearer token → 401.
3. `valid_bearer_token_is_accepted` — token mode, correct
   `Authorization: Bearer` → 200.
4. `valid_session_cookie_is_accepted` — login, then `GET` with the returned
   cookie → 200.
5. `public_routes_need_no_auth` — `/`, `/login`, `/healthz` reachable with no
   credentials in token mode.
6. `mutation_without_custom_header_is_rejected` — authenticated `PUT` without
   `X-Requested-With` → 400.
7. `proxy_mode_accepts_remote_user` — `trust_proxy_auth` true, `Remote-User`
   header → 200; and with the header absent → 401.
8. `open_mode_allows_everything` — empty token, proxy trust false → 200.
9. `no_secret_appears_in_config_payload` — build state with a known SMTP
   password and admin token, `GET /api/config`, assert neither secret string
   appears anywhere in the serialized response body. **This is the invariant
   test.**
10. `token_and_proxy_together_fail_validation` — `Config::validate` returns an
    error (this test lives with the config tests).

No live network tests, no real SMTP, no real GitHub.

## Out of scope

- Manual actions (force-poll-now, send-test-email).
- Multi-user accounts, roles, or per-user sessions.
- TLS termination inside the daemon.
- Hot file-watching of `config.toml`.
- Compiled SPA, npm, or any JS build step.
- Datastar/SSE live updates.
- Trusted-CIDR checks for proxy mode.

## Files touched

1. `Cargo.toml` — add `axum = "0.8"`, `subtle = "2"`, `getrandom = "0.4"`
2. `src/config.rs` — `UiConfig` struct, `[ui]` on `Config`, validation, tests
3. `src/scheduler.rs` — `watch<Arc<Config>>` receive, `changed()` branch,
   status publication, per-send mailer
4. `src/main.rs` — `mod ui;`, config/status watch channels, spawn `ui::serve`,
   startup auth-mode log, mutual-exclusion error path
5. `src/ui/mod.rs` — router, `AppState`, handlers, tests
6. `src/ui/auth.rs` — middleware, session store, login rate limiter
7. `src/ui/assets.rs` — `include_str!` constants
8. `src/ui/index.html` — the page
9. `src/ui/app.js` — the client logic
10. `src/ui/pico.classless.min.css` — vendored Pico 2.1.1 (MIT header kept)
11. `config.example.toml` — document `[ui]`
12. `docker-compose.yml` — config mount rw, `ports: ["127.0.0.1:8080:8080"]`,
    `ADMIN_TOKEN` passthrough
13. `.env.example` — document `ADMIN_TOKEN`
14. `README.md` — UI section, auth modes, security posture, third-party notice,
    `cargo test ui` in the focused-test list
15. `AGENTS.md` — remove `web UI`/`HTTP health endpoint` from out-of-scope, add
    a `## Web UI` section with the invariants
