# Cron Schedule Support — Design Spec

**Date:** 2026-07-10
**Status:** Draft
**Supersedes:** None (extends `2026-07-04-gh-release-notify-design.md`)

## Summary

Add an optional `cron_expression` config field that lets the user specify a
standard 5-field cron expression to control when the daemon polls GitHub for
new releases. The cron expression is **global** (applies to all configured
repos) and takes **precedence** over the existing `poll_interval_seconds`
field when present. When absent, the daemon falls back to the fixed
`poll_interval_seconds` interval (current behavior, unchanged).

## Motivation

A fixed polling interval is simple but inflexible. Users often want
schedule-based control: "check every 6 hours during the day," "check at 9 AM
on weekdays," or "check every 15 minutes during business hours." Cron
expressions are the universal language for this and will be immediately
familiar to homelab users.

## Decisions (from brainstorming)

| Decision | Choice |
|---|---|
| Config coexistence | `poll_interval_seconds` stays **required**; `cron_expression` is **optional**. If cron is present, it takes precedence; the interval is ignored (but still validated). |
| Cron syntax | Standard 5-field: `minute hour day-of-month month day-of-week`. |
| Missed tick behavior | **Skip missed, align to next.** If the daemon was down or a poll ran long, the next tick is the next future cron occurrence. No catch-up bursts. |
| Timezone | **UTC.** Deterministic, no host TZ dependency. |
| Startup behavior | **Immediate first poll**, then align to cron schedule for subsequent ticks. |
| Implementation crate | `cron = "0.17"` (crates.io). Auto-prepend `"0 "` (seconds) to 5-field expressions to satisfy the crate's 6-field requirement. |

## Architecture

### Current data flow

```
config.toml
  poll_interval_seconds: u64  (required, >= 60)
        │
        ▼
  Config::load() → validate()
        │
        ▼
  scheduler::run(cfg, ...)
    interval = Duration::from_secs(cfg.poll_interval_seconds)  // computed once
    loop {
      poll repos (unchanged body)
      tokio::select! {
        sleep(interval) => {}
        shutdown.changed() => break
      }
    }
```

### New data flow

```
config.toml
  poll_interval_seconds: u64   (required, >= 60, fallback)
  cron_expression: Option<String>  (optional, 5-field cron)
        │
        ▼
  Config::load() → validate()
    if cron_expression is Some:
      normalize to 6 fields (prepend "0 " if 5 fields, or pass @-shorthand through)
      parse with cron::Schedule::from_str
      store parsed Schedule (Option<Schedule>) with #[serde(skip)] on Config
        │
        ▼
  main.rs: log which mode is active
        │
        ▼
  scheduler::run(cfg, ...)
    loop {
      poll repos (unchanged body)
      if let Some(schedule) = &cfg.cron_schedule {
        next = schedule.after(&Utc::now()).next()
        match next {
          Some(dt) => duration = dt - now; sleep(duration) or shutdown
          None => log error, fall back to sleep(interval) for this tick
        }
      } else {
        sleep(interval) or shutdown   // unchanged
      }
    }
```

## Components

### 1. Config (`src/config.rs`)

**New field on `Config`:**

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub poll_interval_seconds: u64,
    pub state_path: String,
    pub sender: String,
    pub repos: Vec<String>,
    pub recipients: Vec<String>,
    pub smtp: SmtpConfig,
    #[serde(default)]
    pub cron_expression: Option<String>,
    #[serde(skip)]
    pub cron_schedule: Option<cron::Schedule>,
}
```

- `cron_expression: Option<String>` — the raw string from TOML, `#[serde(default)]`
  so it's optional (defaults to `None`).
- `cron_schedule: Option<cron::Schedule>` — the parsed schedule, `#[serde(skip)]`
  so it's not deserialized from TOML but populated by `validate()`.

**Validation logic (in `validate()`):**

After the existing validations, if `self.cron_expression` is `Some(expr)`:

1. **Normalize fields:**
   - If `expr` starts with `@` (shorthand like `@daily`, `@hourly`): pass
     through to the parser as-is (the `cron` crate handles these natively).
   - Count whitespace-separated fields. If exactly 5: prepend `"0 "` (seconds
     = 0) to produce a 6-field expression. If exactly 6: use as-is. Any other
     count: `bail!("cron_expression must have 5 or 6 fields, got {n}")`.
2. **Parse:** `Schedule::from_str(&normalized)`. On error:
   `bail!("invalid cron_expression '{expr}': {e}")`.
3. **Store:** `self.cron_schedule = Some(schedule)`.

`poll_interval_seconds` validation (`>= 60`) remains unchanged and always
runs, even when cron is present. This ensures the fallback value is always
valid.

### 2. Scheduler (`src/scheduler.rs`)

The `run()` function's poll body is **completely unchanged**. Only the
sleep/wait section at the end of the loop changes.

**Current sleep section (lines 65–72):**

```rust
info!("tick complete, sleeping {}s", cfg.poll_interval_seconds);
tokio::select! {
    _ = tokio::time::sleep(interval) => {}
    _ = shutdown.changed() => {
        info!("shutdown signaled, exiting scheduler");
        break;
    }
}
```

**New sleep section:**

`DateTime<Utc>` has no direct conversion to `tokio::time::Instant` (Instant is
opaque/monotonic, DateTime is wall-clock). The standard interop pattern is to
compute the `Duration` from now to the next occurrence and sleep for that
duration:

```rust
if let Some(schedule) = &cfg.cron_schedule {
    let now = chrono::Utc::now();
    match schedule.after(&now).next() {
        Some(next_dt) => {
            let duration = (next_dt - now).to_std().unwrap_or_default();
            info!("tick complete, next poll at {next_dt}");
            tokio::select! {
                _ = tokio::time::sleep(duration) => {}
                _ = shutdown.changed() => {
                    info!("shutdown signaled, exiting scheduler");
                    break;
                }
            }
        }
        None => {
            error!("cron schedule has no future occurrences, falling back to poll_interval_seconds");
            info!("tick complete, sleeping {}s", cfg.poll_interval_seconds);
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = shutdown.changed() => {
                    info!("shutdown signaled, exiting scheduler");
                    break;
                }
            }
        }
    }
} else {
    info!("tick complete, sleeping {}s", cfg.poll_interval_seconds);
    tokio::select! {
        _ = tokio::time::sleep(interval) => {}
        _ = shutdown.changed() => {
            info!("shutdown signaled, exiting scheduler");
            break;
        }
    }
}
```

**Note on `to_std().unwrap_or_default()`:** `chrono::Duration::to_std()`
returns `Err` if the duration is negative (next occurrence is in the past,
which can happen if the clock advanced between `after()` and the conversion).
`unwrap_or_default()` yields a zero `Duration`, causing an immediate tick —
which is the correct "skip missed, align to next" behavior: the loop runs
again, `after(&now)` returns the next future occurrence, and we sleep until
then.

**Key behaviors:**
- **Immediate first poll:** The loop body runs before any sleep, so the first
  poll is immediate regardless of mode (current behavior preserved).
- **Skip missed, align to next:** `schedule.after(&now)` returns the next
  future occurrence after `now`. If a poll ran long and the scheduled time
  passed, `after()` naturally skips to the next future time. No catch-up.
- **None fallback:** If the schedule has no future occurrences (edge case),
  fall back to `poll_interval_seconds` for that tick and log an error.
- **Shutdown:** The `tokio::select!` with `shutdown.changed()` is identical in
  all branches.

### 3. Main (`src/main.rs`)

**Startup log line update (lines 40–50):**

```rust
if cfg.cron_schedule.is_some() {
    info!(
        "config loaded: {} repos, cron schedule '{}', state path {}, token {}",
        cfg.repos.len(),
        cfg.cron_expression.as_deref().unwrap_or(""),
        cfg.state_path,
        if cfg.github_token().is_some() { "present" } else { "absent" }
    );
} else {
    info!(
        "config loaded: {} repos, poll interval {}s, state path {}, token {}",
        cfg.repos.len(),
        cfg.poll_interval_seconds,
        cfg.state_path,
        if cfg.github_token().is_some() { "present" } else { "absent" }
    );
}
```

No other changes to `main.rs` — the scheduler receives `Config` as before;
`cron_schedule` is already populated by `validate()` during `Config::load()`.

### 4. Dependencies (`Cargo.toml`)

Add:

```toml
cron = "0.17"
```

The `cron` crate depends on `chrono` (with `clock` feature), which is already
present. No conflicts with existing dependencies.

### 5. Documentation

**`config.example.toml`** — add after `poll_interval_seconds`:

```toml
# How often to poll GitHub, in seconds. Minimum 60.
poll_interval_seconds = 3600

# Optional: cron expression to control poll timing. If present, takes
# precedence over poll_interval_seconds. Standard 5-field syntax:
#   minute hour day-of-month month day-of-week
# Examples:
#   "0 */6 * * *"      every 6 hours
#   "0 9 * * 1-5"      09:00 on weekdays
#   "*/15 * * * *"     every 15 minutes
# Timezone is UTC. Day-of-week: 1=Sunday .. 7=Saturday.
# Shorthand: @hourly, @daily, @weekly, @monthly, @yearly
# cron_expression = "0 */6 * * *"
```

**`README.md`** — update the config snippet (line 24) to show the optional
`cron_expression` field, and add a note in the "What it does" section about
cron scheduling being available.

**`AGENTS.md`** — update the "Config & env" section to mention
`cron_expression`.

## Error Handling

| Case | Handling |
|---|---|
| Invalid cron syntax | `bail!` at config load with `"invalid cron_expression '{expr}': {parse_error}"`. Daemon refuses to start. |
| Wrong field count (1–4 or 7+) | `bail!` with `"cron_expression must have 5 or 6 fields, got {n}"`. |
| `@`-shorthand | Pass through to `cron` parser directly (skip field count check). |
| No future occurrences | Log error, fall back to `poll_interval_seconds` for that tick. |
| `poll_interval_seconds < 60` | Unchanged: `bail!` at config load. Always validated, even when cron is present. |

## Edge Cases & Notes

- **Sunday = 1:** The `cron` crate uses 1=Sunday through 7=Saturday. This
  differs from some cron implementations where 0 or 7 = Sunday. Documented in
  `config.example.toml`. No code workaround.
- **6-field expressions accepted:** If a user provides a 6-field expression
  (with seconds), it's used as-is. This is a convenience for users who know
  the crate's native syntax.
- **`poll_interval_seconds` still required:** Even when cron is present, the
  interval must be valid. It serves as the fallback for the no-future-
  occurrence edge case and keeps the config backward-compatible.

## Testing

Unit tests in `config.rs` under `#[cfg(test)]`:

1. `parses_valid_config_with_cron` — config with both fields, asserts
   `cron_schedule` is `Some` and `cron_expression` is the raw string.
2. `parses_config_without_cron` — existing valid config (no cron field),
   asserts `cron_schedule` is `None`. (Existing `parses_valid_config`
   already covers this; verify it still passes.)
3. `rejects_invalid_cron_expression` — `cron_expression = "not a cron"`,
   asserts error contains `"cron_expression"`.
4. `rejects_cron_wrong_field_count` — `cron_expression = "0 9 *"` (3 fields),
   asserts error mentions field count.
5. `auto_prepends_seconds_for_5_fields` — `cron_expression = "0 */6 * * *"`
   (5 fields), asserts `cron_schedule` is `Some` (parse succeeded).
6. `accepts_6_field_cron` — `cron_expression = "0 0 */6 * * *"` (6 fields),
   asserts `cron_schedule` is `Some`.
7. `accepts_cron_shorthand` — `cron_expression = "@daily"`, asserts
   `cron_schedule` is `Some`.

**No scheduler tests.** The scheduler is documented as "glue; its behavior is
exercised manually." The cron scheduling logic is a thin wrapper over the
well-tested `cron` crate's `after().next()`. Config-level parsing tests cover
the validation path.

**Manual smoke test:** Run with `cron_expression = "*/2 * * * *"` (every 2
minutes) and `poll_interval_seconds = 60` fallback. Verify the daemon polls
at 2-minute intervals aligned to cron (at minute boundaries), not every 60
seconds.

## Out of Scope

- Per-repo cron expressions (cron is global, applies to all repos).
- Timezone configuration (UTC only).
- Catch-up for missed ticks (skip-and-align only).
- Runtime cron re-validation (parsed once at load).
- Web UI, health endpoint, HTML email, pre-release notifications (already out
  of scope per project rules).

## Files Touched

1. `Cargo.toml` — add `cron = "0.17"`
2. `src/config.rs` — add `cron_expression` + `cron_schedule` fields, validation
   logic, unit tests
3. `src/scheduler.rs` — add cron scheduling branch in `run()` sleep section
4. `src/main.rs` — update startup log line to reflect active mode
5. `config.example.toml` — document `cron_expression` field
6. `README.md` — document `cron_expression` in config snippet and "What it
   does" section
7. `AGENTS.md` — mention `cron_expression` in Config & env section