# Configurable From display name — Design Spec

**Date:** 2026-10-11
**Status:** Draft (awaiting approval)
**Author:** j1mm0 (with opencode)

## 1. Overview

gh-release-notify currently sends notifications with a bare `From` address
(e.g. `gh-release-notify@homelab.local`). Mail clients then show the raw
address as the sender. This feature adds an optional, configurable **From
display name** so the recipient's client shows a friendly name instead, e.g.:

```
From: gh-release-notify <gh-release-notify@homelab.local>
```

The display name is configured once, applies to every notification, and is
strictly additive: when it is absent, the daemon behaves exactly as it does
today.

### Objective

- Let the operator set the human-readable sender name shown in the email
  client, without changing the underlying `From` address.
- Keep the change backward compatible and the config surface minimal.

### Success criteria

- With `sender_name` set, the sent message's `From` header renders as
  `<name> <sender>`.
- With `sender_name` absent, the `From` header is byte-for-byte today's bare
  address.
- Non-ASCII names are accepted and RFC 2047-encoded by `lettre`.
- Invalid names (empty, whitespace-only, containing CR/LF or other control
  characters, or over 128 characters) fail fast at startup with a clear,
  field-specific error.
- The verification gate (`cargo fmt --check`, `cargo clippy -- -D warnings`,
  `cargo test`) passes with new unit tests covering name validation and
  named-mailbox construction.

## 2. Scope

### In scope

- New optional top-level config key `sender_name`.
- Validation of `sender_name` (see §4).
- Building the `From` mailbox (address + optional name) once at startup.
- Updating `config.example.toml`, `README.md`, and this spec's sibling design
  spec as needed.
- Unit tests in `config.rs` and `notify.rs`.

### Out of scope (YAGNI)

- Customizing the recipient (`To`) display names.
- Per-repo override of the display name.
- Folding the name into the `sender` string (`"Name <addr>"`).
- Customizing the subject prefix, `Reply-To`, or other headers.
- HTML/multipart changes (still plain text only).
- New environment-variable overrides for the name.

## 3. Configuration Schema (delta)

One new optional key is added to the top-level table:

```toml
# Email "From" address for notifications.
sender = "gh-release-notify@homelab.local"

# Optional display name shown as the sender in the email client.
# When omitted, the bare address is used.
sender_name = "gh-release-notify"
```

- Rust representation: `pub sender_name: Option<String>` on `Config`, with
  `#[serde(default)]` so existing configs without the key keep parsing.
- No rename/alias; TOML key matches the Rust field name, per existing
  convention.
- `sender` and `recipients` are unchanged and retain their strict bare-email
  validation.

## 4. Validation Rules

Added to `Config::validate`:

- If `sender_name` is `Some`, the **raw** value is first checked for control
  characters (see below); it is then **trimmed** and the trimmed value is
  stored back.
- The raw value MUST NOT contain any control character (`char::is_control`,
  i.e. Unicode category Cc — ASCII controls including `\r`, `\n`, `\t`, plus
  C1 and DEL). This check runs **before** trimming, so a trailing `\n`/`\r`
  (e.g. `"Attacker\n"`) is rejected rather than silently stripped. Rationale:
  a CR/LF in a header value is header injection; reject it explicitly rather
  than relying on the email encoder. Ordinary spaces (U+0020) are not control
  characters and are trimmed.
- The trimmed value MUST be non-empty (an explicitly empty or whitespace-only
  `sender_name` is an error, not a silent "no name").
- The trimmed value MUST be at most 128 characters (`chars().count()`, after
  trim). Rationale: a generous bound that still prevents absurd/unbounded
  header values.
- Error surfaced through the existing `Config::load` → `anyhow` path with a
  message naming `sender_name`, consistent with the other field validations.

These are the only new rules; all existing validation is untouched.

## 5. Behavior & Implementation

### `notify.rs`

- `Mailer` stores a pre-built `lettre::message::Mailbox` for the sender
  instead of a raw `String`:
  - `Mailer::new(&Config)` parses `cfg.sender` into an `Address`
    (`cfg.sender.parse::<Address>()`, mapping the error to the existing
    `invalid sender '...'` message) and, when `cfg.sender_name` is `Some`,
    wraps it with `Mailbox::new(Some(name), address)`; otherwise
    `Mailbox::new(None, address)`.
  - Building the mailbox at construction time means a malformed sender value
    fails fast at startup rather than at first send.
  - `Mailer` no longer needs the raw `sender: String` field.
- `send_new_release` uses the stored mailbox: `.from(self.from.clone())`.
  The `To` handling, subject, body, headers, and error mapping are unchanged.
- `lettre` 0.11 encodes non-ASCII display names as RFC 2047 words
  automatically; no manual encoding is required.

### Why `Mailbox::new`, not string parsing

Config keeps `sender` and `sender_name` as separate validated fields, so the
mailbox is assembled from already-validated parts (`Mailbox::new`). Parsing a
combined `"Name <addr>"` string is explicitly out of scope (see §2), which
keeps the strict email regex on `sender` intact and avoids brittle string
surgery.

## 6. Documentation

- `config.example.toml`: add the `sender_name` key with a comment, placed
  next to `sender`.
- `README.md`: document `sender_name` in the configuration section.
- `docs/superpowers/specs/2026-07-04-gh-release-notify-design.md`: note the
  optional display name in the configuration schema and component
  description (or record it in that spec's decisions log).

## 7. Testing

Unit tests (in-module `#[cfg(test)]`):

- **`config.rs`:**
  - A config with `sender_name` set parses and the value is populated.
  - A config without `sender_name` parses (backward compatibility) and yields
    `None`.
  - Validation rejects: empty/whitespace-only name; name containing an
    interior `\n`; name containing `\r`; name with a **trailing** `\n`
    (raw-value check); name longer than 128 chars.
  - Validation accepts a name with surrounding whitespace and stores the
    trimmed value, and accepts a name of exactly 128 characters.
- **`notify.rs`:**
  - A helper/test asserts the constructed sender mailbox has the expected
    `name` and `email` when a name is configured, and `name == None` when it
    is not.
  - The rendered `From` header is asserted for the ASCII case
    (`From: gh-release-notify <bot@homelab.local>`) and for a non-ASCII name
    (contains `=?utf-8?`), pinning the RFC 2047 encoding.
  - Existing body tests remain unchanged.

No live SMTP send is performed in tests (consistent with the current suite).

### Verification gate

- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- `cargo test`

## 8. Decisions Log

| Decision              | Choice                                   | Why                                                                                  |
|-----------------------|------------------------------------------|--------------------------------------------------------------------------------------|
| Which mailbox         | `From` only                              | Matches "name shown in the email client"; smallest config surface.                    |
| Config shape          | Separate `sender_name` key               | Keeps strict `sender` email validation; explicit; matches `sender` naming.            |
| Backward compatibility| Optional; bare address when absent       | No silent behavior change for existing deployments.                                   |
| Default name          | None                                     | Avoids surprising existing users.                                                      |
| Validation strictness | Non-empty, no control chars, max 128     | Blocks header injection; bounds header size; clear startup failure.                   |
| Scope                 | Global                                   | Per-repo names already excluded by the project's out-of-scope list.                   |
| Mailbox construction  | Once in `Mailer::new`                     | Fail fast at startup; no per-send parse cost.                                          |

## 9. Open Questions

None.
