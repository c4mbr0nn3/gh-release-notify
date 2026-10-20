# Configurable From Display Name Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional `sender_name` config key so notification emails show a friendly `From` display name in the recipient's mail client. When the key is unset the rendered `From` header is unchanged; a malformed `sender` now fails at startup rather than per-send (spec-sanctioned fail-fast, §5).

**Architecture:** `Config` gains `sender_name: Option<String>` with `#[serde(default)]` and strict validation (trim / non-empty / no control chars / ≤128 chars). `Mailer` builds its `From` `lettre::Mailbox` once in `new()` from `sender` + optional `sender_name` and stores it, replacing the raw `sender: String` field. `send_new_release` uses the stored mailbox. Docs and tests are updated alongside.

**Tech Stack:** Rust 2021, `lettre` 0.11 (Mailbox/Address), `serde`/`toml`, `regex`, `anyhow`, `tempfile` (dev).

**Spec:** `docs/superpowers/specs/2026-10-11-sender-display-name-design.md`

## Global Constraints

- Rust edition 2021. No comments in Rust source files.
- No `unwrap`/`expect`/`panic` in non-test code; all fallible operations return `Result`.
- Config TOML keys are bare snake_case matching Rust field names exactly (no `#[serde(rename)]`).
- `sender` and `recipients` keep their existing strict bare-email validation unchanged; display name is never folded into those strings.
- Commit messages: Conventional Commits, `<type>: <subject>`, ≤72 chars, imperative, subject only (no body).
- Verification gate before any "done" claim, in this exact order: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`.
- The config email regex and lettre's `Address` parser accept slightly different sets (e.g. a 100-char local part passes the regex but fails lettre; `a@b..c` likewise). A regex-valid but lettre-invalid `sender` now fails at startup in `Mailer::new` instead of failing per-send. This is intended fail-fast (spec §5) and is the only behavioral change when `sender_name` is unset.

## Review Focus

Five input classes the spec implies that are most likely to bite, each pinned to a test in its owning task:

1. `sender_name` containing a CR or LF, **including leading/trailing** (e.g. `"Attacker\n"`) — checked on the raw value *before* trimming, so trailing control characters are not silently stripped; must be rejected by config validation, never passed to lettre (Task 1 `rejects_sender_name_with_control_chars`).
2. `sender_name` present but empty or whitespace-only — must error, not silently degrade to a bare address (Task 1 `rejects_empty_sender_name` / `rejects_whitespace_only_sender_name`).
3. `sender_name` longer than 128 characters — must be rejected; exactly 128 is accepted and renders intact (Task 1 `rejects_too_long_sender_name` / `accepts_sender_name_at_128_chars`, Task 2 `from_header_renders_boundary_length_name`).
4. `sender_name` with non-ASCII characters (e.g. `Überwacher`) and with a header-structural special (`"`)— non-ASCII accepted and RFC 2047-encoded; the quote escaped by lettre (Task 1 accept test; Task 2 `non_ascii_display_name_is_rfc2047_encoded` / `from_header_escapes_embedded_quote`).
5. An existing config with no `sender_name` key — must still parse (serde default) and render a bare-address `From` byte-for-byte (Task 1 `sender_name_absent_is_none`, Task 2 `from_header_unchanged_without_name`). The regex-valid/lettre-invalid divergence is pinned by Task 2 `build_sender_rejects_regex_valid_but_lettre_invalid`.

---

### Task 1: Optional `sender_name` config field + validation

**Files:**
- Modify: `src/config.rs` (struct `Config` at lines 7-19; `validate` at lines 59-109; tests module at lines 112-355)

**Interfaces:**
- Consumes: nothing new.
- Produces: `Config.sender_name: Option<String>` (public field, `#[serde(default)]`), populated (trimmed) by `Config::validate`. Malformed names cause `Config::load` to return `Err` whose message contains `sender_name`.

- [ ] **Step 1: Write the failing tests**

Add these tests to the `mod tests` block in `src/config.rs` (after `rejects_bad_sender_email`, ~line 261):

```rust
    fn with_sender_name(name_line: &str) -> String {
        VALID.replace(
            "sender = \"bot@homelab.local\"",
            &format!("sender = \"bot@homelab.local\"\n{name_line}"),
        )
    }

    #[test]
    fn parses_sender_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_config(dir.path(), &with_sender_name("sender_name = \"gh-release-notify\""));
        let cfg = Config::load(p.to_str().unwrap()).unwrap();
        assert_eq!(cfg.sender_name.as_deref(), Some("gh-release-notify"));
    }

    #[test]
    fn sender_name_absent_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_config(dir.path(), VALID);
        let cfg = Config::load(p.to_str().unwrap()).unwrap();
        assert!(cfg.sender_name.is_none());
    }

    #[test]
    fn trims_sender_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_config(
            dir.path(),
            &with_sender_name("sender_name = \"  gh-release-notify  \""),
        );
        let cfg = Config::load(p.to_str().unwrap()).unwrap();
        assert_eq!(cfg.sender_name.as_deref(), Some("gh-release-notify"));
    }

    #[test]
    fn accepts_non_ascii_sender_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_config(dir.path(), &with_sender_name("sender_name = \"Überwacher\""));
        let cfg = Config::load(p.to_str().unwrap()).unwrap();
        assert_eq!(cfg.sender_name.as_deref(), Some("Überwacher"));
    }

    #[test]
    fn rejects_empty_sender_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_config(dir.path(), &with_sender_name("sender_name = \"\""));
        let err = Config::load(p.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("sender_name"));
    }

    #[test]
    fn rejects_whitespace_only_sender_name() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_config(dir.path(), &with_sender_name("sender_name = \"   \""));
        let err = Config::load(p.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("sender_name"));
    }

    #[test]
    fn rejects_sender_name_with_control_chars() {
        for raw in ["bad\\nname", "bad\\rname", "bot\\n", "\\tbot"] {
            let dir = tempfile::tempdir().unwrap();
            let p = write_config(
                dir.path(),
                &with_sender_name(&format!("sender_name = \"{raw}\"")),
            );
            let err = Config::load(p.to_str().unwrap()).unwrap_err();
            assert!(
                err.to_string().contains("sender_name"),
                "control-char name {raw:?} was not rejected"
            );
        }
    }

    #[test]
    fn accepts_sender_name_at_128_chars() {
        let dir = tempfile::tempdir().unwrap();
        let exact = "a".repeat(128);
        let p = write_config(
            dir.path(),
            &with_sender_name(&format!("sender_name = \"{exact}\"")),
        );
        let cfg = Config::load(p.to_str().unwrap()).unwrap();
        assert_eq!(cfg.sender_name.as_deref(), Some(exact.as_str()));
    }

    #[test]
    fn rejects_too_long_sender_name() {
        let dir = tempfile::tempdir().unwrap();
        let long = "a".repeat(129);
        let p = write_config(
            dir.path(),
            &with_sender_name(&format!("sender_name = \"{long}\"")),
        );
        let err = Config::load(p.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("sender_name"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test config`
Expected: compile error — every new test fails with E0609 (`no field 'sender_name' on type 'Config'`), including the accept tests, because the field does not exist yet.

- [ ] **Step 3: Add the `sender_name` field to `Config`**

In `src/config.rs`, inside `struct Config` (after `pub sender: String,` at line 11), add:

```rust
    #[serde(default)]
    pub sender_name: Option<String>,
```

- [ ] **Step 4: Add validation in `Config::validate`**

In `src/config.rs`, immediately after the `sender` email check (the `if !email_re.is_match(&self.sender) { ... }` block ending at line 78), add:

```rust
        if let Some(name) = self.sender_name.take() {
            if name.chars().any(char::is_control) {
                bail!("sender_name must not contain control characters");
            }
            let trimmed = name.trim();
            if trimmed.is_empty() {
                bail!("sender_name must not be empty");
            }
            if trimmed.chars().count() > 128 {
                bail!("sender_name must be at most 128 characters");
            }
            self.sender_name = Some(trimmed.to_string());
        }
```

Order matters: the control-character check runs on the **raw** value before trimming, so a trailing `\n`/`\r` is rejected rather than silently stripped. `char::is_control` is Unicode category Cc (ASCII controls plus C1 and DEL); ordinary surrounding spaces (U+0020) are not control characters and are trimmed.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test config`
Expected: PASS (all config tests, including the new ones).

- [ ] **Step 6: Run clippy and fmt**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: no warnings; all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/config.rs
git commit -m "feat: add optional sender_name config field"
```

---

### Task 2: Build the named `From` mailbox in `Mailer`

**Files:**
- Modify: `src/notify.rs` (imports at lines 2-5; `Mailer` struct at lines 29-32; `Mailer::new` at lines 35-58; `send_new_release` at lines 60-94; tests module at lines 97-133)

**Interfaces:**
- Consumes: `Config.sender_name: Option<String>` and `Config.sender: String` from Task 1.
- Produces: `pub fn build_sender(sender: &str, name: Option<&str>) -> Result<Mailbox>` (public for symmetry with the sibling `pub fn build_body`); `Mailer` now holds `from: Mailbox` (private field) instead of `sender: String`. `send_new_release` signature is unchanged.

- [ ] **Step 1: Write the failing tests**

Add these tests inside the `mod tests` block in `src/notify.rs` (after `body_falls_back_to_tag_when_name_is_null`, ~line 132):

```rust
    #[test]
    fn sender_mailbox_includes_display_name() {
        let m = build_sender("bot@homelab.local", Some("gh-release-notify")).unwrap();
        assert_eq!(m.name.as_deref(), Some("gh-release-notify"));
        assert_eq!(m.email.to_string(), "bot@homelab.local");
    }

    #[test]
    fn sender_mailbox_without_name_has_none() {
        let m = build_sender("bot@homelab.local", None).unwrap();
        assert_eq!(m.name, None);
        assert_eq!(m.email.to_string(), "bot@homelab.local");
    }

    #[test]
    fn sender_mailbox_accepts_non_ascii_name() {
        let m = build_sender("bot@homelab.local", Some("Überwacher")).unwrap();
        assert_eq!(m.name.as_deref(), Some("Überwacher"));
    }

    #[test]
    fn build_sender_rejects_regex_valid_but_lettre_invalid() {
        let long_local = format!("{}@example.com", "a".repeat(100));
        let err = build_sender(&long_local, None).unwrap_err();
        assert!(err.to_string().contains("invalid sender"));
    }

    fn rendered_from_header(mailbox: Mailbox) -> String {
        let to: Mailbox = "you@example.com".parse().unwrap();
        let email = Message::builder()
            .from(mailbox)
            .to(to)
            .subject("subject")
            .body("body".to_string())
            .unwrap();
        email.headers().to_string()
    }

    #[test]
    fn from_header_renders_display_name() {
        let m = build_sender("bot@homelab.local", Some("gh-release-notify")).unwrap();
        let rendered = rendered_from_header(m);
        assert!(
            rendered.contains("From: gh-release-notify <bot@homelab.local>"),
            "rendered headers were: {rendered}"
        );
    }

    #[test]
    fn non_ascii_display_name_is_rfc2047_encoded() {
        let m = build_sender("bot@homelab.local", Some("Überwacher")).unwrap();
        let rendered = rendered_from_header(m);
        assert!(
            rendered.contains("=?utf-8?"),
            "expected RFC 2047 encoded display name, got: {rendered}"
        );
    }

    #[test]
    fn from_header_unchanged_without_name() {
        let m = build_sender("bot@homelab.local", None).unwrap();
        let rendered = rendered_from_header(m);
        assert!(
            rendered.contains("From: bot@homelab.local\r\n"),
            "unset-name From header changed: {rendered}"
        );
    }

    #[test]
    fn from_header_escapes_embedded_quote() {
        let m = build_sender("bot@homelab.local", Some("Foo \" Bar")).unwrap();
        let rendered = rendered_from_header(m);
        assert!(
            rendered.contains("\\\""),
            "expected the embedded quote to be escaped, got: {rendered}"
        );
    }

    #[test]
    fn from_header_renders_boundary_length_name() {
        let name = "a".repeat(128);
        let m = build_sender("bot@homelab.local", Some(&name)).unwrap();
        let rendered = rendered_from_header(m);
        assert!(
            rendered.contains(&name),
            "128-char display name was not rendered intact"
        );
        assert!(rendered.contains("<bot@homelab.local>"));
    }
```

`Message::headers() -> &Headers` and `impl Display for Headers` are public lettre 0.11.23 API (verified in the pinned crate source; `Headers` writes `\r\n` line endings), so the rendered `From` line is asserted directly. The overlong-local-part input in `build_sender_rejects_regex_valid_but_lettre_invalid` passes the config regex but fails lettre's 64-char local-part limit (`check_user`), pinning the fail-fast divergence.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test notify`
Expected: compile error — `build_sender` not found.

- [ ] **Step 3: Extend the imports**

In `src/notify.rs`, replace the `lettre` import block (lines 2-5) with:

```rust
use lettre::{
    message::{header::ContentType, Mailbox},
    transport::smtp::authentication::Credentials,
    Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};
```

- [ ] **Step 4: Add the `build_sender` helper**

In `src/notify.rs`, after `build_body` (before `pub struct Mailer`), add:

```rust
pub fn build_sender(sender: &str, name: Option<&str>) -> Result<Mailbox> {
    let address: Address = sender
        .parse()
        .map_err(|e| anyhow!("invalid sender '{sender}': {e}"))?;
    Ok(Mailbox::new(name.map(str::to_string), address))
}
```

- [ ] **Step 5: Change `Mailer` to store the `From` mailbox**

In `src/notify.rs`, replace the struct (lines 29-32):

```rust
pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}
```

Then, in `Mailer::new`, replace the returned struct initialization (lines 54-57):

```rust
        Ok(Mailer {
            transport: builder.build(),
            from: build_sender(&cfg.sender, cfg.sender_name.as_deref())?,
        })
```

- [ ] **Step 6: Use the stored mailbox in `send_new_release`**

In `src/notify.rs`, replace the `from(...)` construction (lines 69-73) with:

```rust
        let mut builder = Message::builder().from(self.from.clone());
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test notify`
Expected: PASS (all notify tests, including the new ones).

- [ ] **Step 8: Run the full gate**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: no warnings; all tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/notify.rs
git commit -m "feat: use sender display name in notification From header"
```

---

### Task 3: Documentation

**Files:**
- Modify: `config.example.toml` (sender section, lines 21-22)
- Modify: `README.md` (config block, lines 23-41)
- Modify: `docs/superpowers/specs/2026-07-04-gh-release-notify-design.md` (config schema §4 line 91 + validation list §4; decisions log §9)

**Interfaces:**
- Consumes: the `sender_name` key and its semantics from Tasks 1-2.
- Produces: no code; documentation only.

- [ ] **Step 1: Update `config.example.toml`**

After the `sender = "gh-release-notify@homelab.local"` line (line 22), add:

```toml
# Optional display name shown as the sender in the email client.
# When omitted, only the bare address is shown.
# sender_name = "gh-release-notify"
```

- [ ] **Step 2: Update `README.md`**

In the TOML block, after `sender = "gh-release-notify@homelab.local"` (line 26), add:

```toml
sender_name = "gh-release-notify"  # optional; display name shown in email clients
```

- [ ] **Step 3: Update the design spec**

In `docs/superpowers/specs/2026-07-04-gh-release-notify-design.md`:

- In §4, after the `sender = ...` line (line 91), insert:

```toml
# Optional display name shown as the sender in the email client.
sender_name = "gh-release-notify"
```

- In §4's "Validation rules" list (under line 122, `sender` is a valid email), add this bullet:

```markdown
- `sender_name` (optional) must be non-empty after trimming, contain no control characters (checked before trimming), and be at most 128 characters.
```

- In §9 Decisions Log, append **only this row** to the existing table (do not re-add the header or separator lines — the table already has them):

```markdown
| Sender display name | Optional `sender_name` | Friendly sender shown in mail clients; rendered `From` header unchanged when unset. |
```

- [ ] **Step 4: Verify the gate**

Run: `cargo fmt && cargo clippy -- -D warnings && cargo test`
Expected: all pass (docs changes do not affect the build; this confirms nothing regressed).

- [ ] **Step 5: Commit**

```bash
git add config.example.toml README.md docs/superpowers/specs/2026-07-04-gh-release-notify-design.md
git commit -m "docs: document sender_name config key"
```

---

## Self-Review

**1. Spec coverage:** Config key + validation → Task 1 (including raw-value control-char check, exact-128 boundary, trailing-newline rejection). Mailbox construction / fail-fast / send path → Task 2. Rendered `From` header and non-ASCII RFC 2047 encoding → pinned by Task 2 `from_header_renders_display_name` and `non_ascii_display_name_is_rfc2047_encoded`, which assert the rendered header string rather than only struct storage. Docs (config.example.toml, README, sibling spec) → Task 3. All spec sections map to a task.

**2. Step scan:** Each step is one action with a checkable result. No "TBD" / "handle edge cases" lines. Code steps give exact signatures and validation values, not full bodies (the trivial bodies are fully determined and shown where the spec pins exact values).

**3. Type consistency:** `Config.sender_name: Option<String>` used identically in Task 1 and Task 2. `build_sender(&str, Option<&str>) -> Result<Mailbox>` is defined and called with `cfg.sender_name.as_deref()`. `Mailer.from: Mailbox` is written in `new` and read in `send_new_release`.

**4. Review Focus:** All five listed inputs have a test in the owning task (see the per-item test names in Review Focus), including the backward-compat rendered-header case, the regex-valid/lettre-invalid divergence, the embedded-quote escape, and the 128-char render boundary.

**6. Council review:** pragmatist (PROCEED, trims), architect (PROCEED), skeptic (CONCERNS — additive tests only) reconciled: control-char reject tests consolidated into one table-driven test; added `from_header_unchanged_without_name`, `build_sender_rejects_regex_valid_but_lettre_invalid`, `from_header_escapes_embedded_quote`, `from_header_renders_boundary_length_name`; `build_sender` made `pub` to match `build_body`; the fragile §3 sibling-spec prose edit dropped. Tasks 1/2 kept as separate commits (tag-driven releases never deploy an intermediate commit).

**5. Proportion:** Plan is comparable in size to the spec; code blocks are test bodies and the few exact values the spec pins, not a transcript of the implementation.

## Execution Handoff

Tasks are tightly coupled (Task 2 depends on Task 1's field; Task 3 documents both), three tasks total, low shipped-mistake cost. Recommended execution: **Native** (`executing-plans`) — implement in this session, single fresh whole-branch review at the end.
