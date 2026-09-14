# CONFIG-WRITEBACK — Design Spec

**Date:** 2026-09-14
**Status:** Approved
**Module id:** `config-writeback`
**Supersedes:** None (extends `2026-07-04-gh-release-notify-design.md`)
**Depends on:** Nothing. `web-ui` depends on this module.

## Summary

Give the daemon the ability to rewrite its own `config.toml` at runtime from
a validated in-memory `Config`, without destroying the comments, key order, or
formatting the user hand-authored. Writes are atomic, and an invalid result can
never reach the file.

Today `Config` is read-only by construction: it derives `Deserialize` only
(`src/config.rs:7`), `Config::load` only ever calls `read_to_string`
(`src/config.rs:40`), and no code path in `src/` writes to the config path.
This module adds exactly one new capability — a validated write — and nothing
else.

## Motivation

The UI's entire purpose is editing interval, cron, and tracked repos. Those
edits must land in the file the user reads and maintains. Serializing the
struct naively through `toml::to_string` would strip every explanatory comment
from `config.example.toml`'s descendants, which is hostile to the operator and
would make the UI unusable in practice for a file this heavily commented.

## Decisions (from design grilling)

| Decision | Choice |
|---|---|
| Write mechanism | `toml_edit::DocumentMut` — preserves comments, whitespace, and key order |
| Atomicity | Write `<path>.tmp`, then `std::fs::rename` — same discipline as `state.rs:42-56` |
| Validation ordering | Re-parse the rendered document into `Config` and run `validate()` **before** the rename. Invalid config never touches the target file |
| In-memory source of truth | The daemon's `watch<Arc<Config>>` wins while running. Manual file edits are documented as requiring a restart |
| File watching | None. Explicitly out of scope for v1 |
| Config path retention | The resolved config path is carried on `Config` (new field), since `Config::load` currently discards it (`src/config.rs:39-46`) |
| Repos representation | Plain `["owner/repo", ...]` string array (unchanged from `config.example.toml:25`) — no array-of-tables surgery needed |
| Read-only mount | Detected, surfaced as a typed error, never a panic. The UI renders a banner and disables save |
| Error type | Typed variants, mapped to HTTP status codes by `web-ui` |

## Architecture

### Current write path

```
(does not exist)
```

`Config` has no `Serialize`, `Config::load` discards its own path argument, and
the only filesystem writers in the codebase are `StateStore::save`
(`src/state.rs:42`).

### New write path

```
PUT /api/config  (web-ui module)
      │
      ▼
  ConfigEdit (deserialized request body, secrets optional)
      │
      ▼
  config_writeback::apply(config_path, edit) -> Result<Config, SaveError>
      │
      ├─ 1. read_to_string(config_path)
      ├─ 2. parse::<DocumentMut>()
      ├─ 3. apply edits surgically:
      │       doc["poll_interval_seconds"] = ...
      │       doc["repos"] = array of strings
      │       doc["cron_expression"] = ... or remove
      │       doc["smtp"]["password"] = ... only if a non-empty value was sent
      │       doc["ui"]["admin_token"] = ... only if the field was present (empty = clear)
      ├─ 4. render to string
      ├─ 5. toml::from_str::<Config>(&rendered)   ← re-parse the RESULT
      ├─ 6. cfg.validate()                         ← same validators as startup
      ├─ 7. write "{path}.tmp"
      ├─ 8. rename(tmp, path)                      ← atomic commit point
      └─ 9. return the validated Config
      │
      ▼
  watcher.send(Arc::new(new_config))  ← takes effect next scheduler tick
```

Steps 5–6 are the critical ordering guarantee: **validation happens against the
rendered bytes, before the rename.** A `toml_edit` mutation bug that produces a
document which parses but fails validation is caught here, with the original
file still intact on disk.

## Components

### 1. `Config` gains its own path (`src/config.rs`)

`Config::load` currently takes `path: &str` and drops it
(`src/config.rs:39-46`). Add:

```rust
#[serde(skip)]
pub config_path: String,
```

`#[serde(skip)]` so it is never deserialized from TOML and never rendered back
into the file. `load()` sets it from its argument. `Debug` is derived on
`Config`, so this field's value is visible in debug output — acceptable, it is
a path, not a secret.

Also required: `Encryption` must gain `Serialize` alongside its existing
`Deserialize` (`src/config.rs:30`) so the enum can be written back as a string.
`#[serde(rename_all = "lowercase")]` already produces the correct lowercase
form on the way out.

### 2. New module `src/config_writeback.rs`

Public surface:

```rust
pub enum SaveError {
    Unwritable(String),
    Invalid(String),
    Io(String),
}

pub struct ConfigEdit { /* all non-secret fields + Option<String> secrets */ }

pub fn apply(path: &str, edit: &ConfigEdit) -> Result<Config, SaveError>
```

`SaveError` is a typed enum, not `anyhow::Error`, because `web-ui` must map
variants to distinct HTTP status codes (`Invalid` → 400, `Unwritable` → 409,
`Io` → 500). This mirrors the existing precedent of `GithubError` being a typed
enum with a `Display` impl (`src/github.rs:6-23`).

`ConfigEdit` field groups:

| Group | Fields | Semantics |
|---|---|---|
| Scalars | `poll_interval_seconds`, `sender`, `cron_expression: Option<String>` | Replaced with the submitted value. `cron_expression: None` removes the key |
| Lists | `repos: Vec<String>`, `recipients: Vec<String>` | Replaced wholesale |
| SMTP non-secret | `smtp_host`, `smtp_port`, `smtp_encryption`, `smtp_username` | Replaced with the submitted value |
| Write-only secret | `smtp_password: Option<String>` | `None` or `Some("")` leaves the existing value untouched. `Some(non-empty)` replaces it |
| Write-only secret | `admin_token: Option<String>` | `None` leaves it untouched. `Some("")` explicitly **clears** it (disables login). `Some(non-empty)` replaces it |
| Read-only | `state_path`, `ui_bind_addr`, `ui_port`, `trust_proxy_auth` | Rejected if present in the request body; never written |

The deliberate asymmetry between `smtp_password` and `admin_token` for the
empty string is the only subtle part of this API: an empty SMTP password is
never a meaningful value, so it means "unchanged"; an empty admin token is a
meaningful value ("no login"), so it means "clear". This must be documented in
the UI and asserted by a test.

### 3. Unwritable detection

`Unwritable` is produced when the write or rename fails with
`std::io::ErrorKind::PermissionDenied`, or with `ErrorKind::ReadOnlyFilesystem`
where available. The `docker-compose.yml:15` read-only mount
(`./config.toml:/config/config.toml:ro`) is the primary trigger, but the same
path covers a root-owned file under a non-root container user (uid 10001).

Detection must be a **typed error, never a panic** — the project rule forbids
`unwrap`/`expect`/`panic` in non-test code, and a permissions failure here is
an expected operator configuration, not a bug.

### 4. Dependencies (`Cargo.toml`)

```toml
toml_edit = "0.25"
```

`toml_edit` shares `toml_datetime` / `serde_spanned` internals with the
existing `toml = "1"` (`Cargo.toml:11`), so the marginal package count is
effectively zero. No `Serialize` derive on `Config` itself is needed — the
write path is surgical, not whole-struct serialization.

## Error handling

| Case | `SaveError` | HTTP (by `web-ui`) | Side effect |
|---|---|---|---|
| New config fails parse or `validate()` | `Invalid(msg)` | 400 | File untouched |
| Config file not writable (ro mount, perms) | `Unwritable(msg)` | 409 | File untouched |
| Read/write/rename I/O failure | `Io(msg)` | 500 | tmp file may remain; target untouched |
| Read-only field present in request | `Invalid(msg)` | 400 | File untouched |
| Malformed request body | (axum rejection) | 422 | File untouched |

Crash between step 7 and step 8 leaves a stale `.tmp` beside the config. That
is harmless (the target is intact) and is documented; the next successful save
overwrites it.

## Edge cases & notes

- **Comment preservation is the point.** A naive whole-struct serialization
  (`toml::to_string`) is unacceptable here; a test must assert that a comment
  in the original file survives a write.
- **Key order** is preserved by `toml_edit` for existing keys; a key that did
  not exist (e.g. a first-time `cron_expression`) is appended.
- **`cron_schedule`** is `#[serde(skip)]` (`src/config.rs:17`) and must never
  be written. It is repopulated by `validate()` on the re-parse in step 6, so
  the returned `Config` is immediately usable by the scheduler.
- **No secret is ever read back.** This module writes secrets but has no
  function that returns them; `web-ui` never calls one because none exists.
  This is a structural guarantee, not a convention.
- **Concurrent saves.** Two simultaneous `PUT`s race at the rename. Both are
  validated independently, so the loser's write is simply overwritten —
  last-writer-wins is acceptable for a single-operator tool. No lock is added.

## Testing

Unit tests in `src/config_writeback.rs` under `#[cfg(test)]`, all using the
existing `tempfile` dev-dependency (`Cargo.toml:22`):

1. `preserves_comments_and_order` — write a config containing distinctive
   comments, apply a change to one scalar, assert every comment string and the
   original key order are still present in the file.
2. `invalid_new_config_is_rejected_before_write` — apply an edit that produces
   an invalid config (e.g. `poll_interval_seconds = 30`); assert the returned
   error is `Invalid`, and assert the on-disk file is **byte-identical** to the
   original.
3. `save_is_atomic_and_leaves_no_tmp` — after a successful apply, assert no
   `<path>.tmp` remains and the new content is readable.
4. `empty_smtp_password_leaves_secret_untouched` — file has `password = "x"`;
   apply an edit with `smtp_password: Some("")`; assert the file still contains
   `x`.
5. `admin_token_empty_clears_value` — file has a non-empty `admin_token`;
   apply with `Some("")`; assert the key is now empty (the differing semantics
   from test 4, asserted explicitly).
6. `absent_secret_fields_leave_values_untouched` — `None` for both secrets
   changes neither key.
7. `round_trips_through_config_validate` — the returned `Config` has its edits
   applied and `cron_schedule` populated when cron was set.
8. `unwritable_path_returns_unwritable_not_panic` — point `apply` at a
   read-only directory (mode 0o555 via `std::fs::set_permissions`); assert
   `SaveError::Unwritable`. Skipped on platforms where this cannot be set up.
9. `rejects_read_only_fields` — an edit attempting `state_path` or
   `ui_bind_addr` returns `Invalid`.

No HTTP, no network, no async runtime needed — these are pure filesystem
tests.

## Out of scope

- Hot file-watching / reload of externally edited config.
- Writing `state_path`, `ui.bind_addr`, `ui.port`, `trust_proxy_auth`.
- Multiple config file formats or a database-backed config store.
- Config versioning, history, or rollback.
- Encrypting secrets at rest.

## Files touched

1. `Cargo.toml` — add `toml_edit = "0.25"`
2. `src/config.rs` — add `config_path` field, `Serialize` on `Encryption`,
   set path in `load()`, unit tests for the new field
3. `src/config_writeback.rs` — new module: `ConfigEdit`, `SaveError`, `apply()`,
   unit tests
4. `src/main.rs` — declare `mod config_writeback;`
