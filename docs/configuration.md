# Configuration

The daemon reads a TOML file. The path comes from `--config`, the
`CONFIG_PATH` environment variable, or the default `./config.toml`.
[`config.example.toml`](../config.example.toml) is a fully commented starting
point.

## Top-level keys

| Key | Type | Required | Notes |
|---|---|---|---|
| `poll_interval_seconds` | integer | yes | Minimum `60`. Used when `cron_expression` is absent. |
| `cron_expression` | string | no | Takes precedence over the interval. See [Cron](#cron). |
| `state_path` | string | yes | JSON file of last-seen tags. See [State path](#state-path). |
| `sender` | string | yes | Valid email address used in the `From` header. |
| `sender_name` | string | no | Display name; trimmed, at most 128 characters, no control characters. |
| `repos` | array of strings | yes | Non-empty; each entry is `owner/repo`. |
| `recipients` | array of strings | yes | Non-empty; each entry must be a valid email address. |

## `[smtp]`

| Key | Type | Required | Notes |
|---|---|---|---|
| `host` | string | yes | Non-empty SMTP hostname. |
| `port` | integer | yes | `1`–`65535`. |
| `encryption` | string | yes | `starttls` (usually 587), `tls` (implicit TLS, usually 465), or `none` (usually 25). |
| `username` | string | yes | May be empty. If set, a password is required. |
| `password` | string | yes | May be empty when `username` is empty. Prefer `SMTP_PASSWORD`. |

## `[ui]`

The section is optional; defaults apply when it is absent.

| Key | Default | Editable in UI | Notes |
|---|---|---|---|
| `bind_addr` | `127.0.0.1` | no | Must be a valid IP address. `UI_BIND_ADDR` overrides it at startup. |
| `port` | `8080` | no | Must be greater than zero. `UI_PORT` overrides it at startup. |
| `admin_token` | empty | yes | Empty means no token login. Prefer the `ADMIN_TOKEN` variable. |
| `trust_proxy_auth` | `false` | no | Accepts the `Remote-User` header. Mutually exclusive with a token. |

Setting both `admin_token` and `trust_proxy_auth = true` is a **startup error**.
`bind_addr`, `port`, `trust_proxy_auth`, and `state_path` are rejected by the UI
as read-only; changing them requires editing the file or environment and
restarting.

## Environment variables

Overrides apply only when the value is non-empty. An empty value falls through
to the config file.

| Variable | Purpose | Default |
|---|---|---|
| `CONFIG_PATH` | Config file path (also `--config`). | `./config.toml` |
| `STATE_PATH` | Overrides `state_path` (also `--state-path`). An empty value is a startup error. | config value |
| `GITHUB_TOKEN` | GitHub token sent as `Authorization: Bearer`. Raises the API rate limit from 60 to 5,000 requests/hour. | unset |
| `SMTP_PASSWORD` | Overrides `[smtp].password`; keeps the secret out of the file. | config value |
| `ADMIN_TOKEN` | Overrides `[ui].admin_token`. | config value |
| `UI_BIND_ADDR` | Overrides `[ui].bind_addr` at startup. An empty value is a startup error. Never written back. | config value |
| `UI_PORT` | Overrides `[ui].port` at startup. An empty or invalid value is a startup error. Never written back. | config value |
| `RUST_LOG` | `tracing` filter directive. | `info` |

`.env.example` documents the secret-bearing variables. Docker Compose reads a
`.env` file in the project directory automatically; the daemon itself does not
load `.env`. For a local `cargo run`, export the variables in your shell.

## State path

`state_path` is a JSON object mapping `owner/repo` to the last-seen tag, for
example:

```json
{
  "fosrl/pangolin": "1.24.0",
  "fosrl/newt": "1.18.1"
}
```

- Docker/compose deployments must use `/state/state.json`, the mounted volume.
- Local runs usually use `./state.json`.
- The parent directory must exist; a missing parent produces a state-save error.
- Precedence is `--state-path` > `STATE_PATH` > the config file. The override is
  runtime-only and is never written back to the file.

## Cron

`cron_expression` accepts a standard 5-field expression
(`minute hour day-of-month month day-of-week`), a 6-field expression with
seconds, or a shorthand such as `@hourly`, `@daily`, `@weekly`, `@monthly`, or
`@yearly`.

- The timezone is UTC.
- A 5-field expression gets a leading `0` seconds field automatically.
- Day-of-week follows the `cron` crate convention: **1 = Sunday … 7 =
  Saturday**. This differs from implementations where 0 or 1 means Monday.
- When present, the cron schedule takes precedence over
  `poll_interval_seconds`.

Examples:

```toml
cron_expression = "0 */6 * * *"    # every 6 hours
cron_expression = "0 9 * * 1-5"    # 09:00 UTC, Monday to Friday
cron_expression = "*/15 * * * *"   # every 15 minutes
```

## Validation

Startup validation rejects:

- `poll_interval_seconds < 60`
- a cron expression with the wrong field count or invalid syntax
- an empty `repos` or `recipients` list
- an invalid `sender` or recipient email
- an invalid `owner/repo` entry
- `smtp.port == 0` or an empty `smtp.host`
- `smtp.username` set without a password
- an invalid or empty `ui.bind_addr`, `ui.port == 0`
- `admin_token` and `trust_proxy_auth = true` together
- `sender_name` empty after trimming, longer than 128 characters, or containing
  control characters

## UI edits

The web UI writes through `config_writeback::apply`: `toml_edit` preserves
comments and key order, the rendered file is re-parsed and validated, then it is
written atomically with a temporary file and rename. A validation failure leaves
the original file untouched. See [Web UI & auth](web-ui.md).
