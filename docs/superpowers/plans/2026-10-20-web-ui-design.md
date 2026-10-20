# Web UI Design Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the framework-default page into a designed control panel: a dense, machine-typographic homelab dashboard with a status band, grouped settings, a reachable Save, and a phone composition that is its own layout rather than a narrowed desktop.

**Architecture:** Add one authored stylesheet (`src/ui/ui.css`) embedded with `include_str!` and served at `/ui.css`, layered over the vendored Pico classless sheet: Pico keeps form controls, focus rings and dark mode; the new sheet owns layout, hierarchy, type scale, control emphasis and state colour. Markup gains one admin-access group, a sticky save bar, and a status metadata line; `app.js` gains dirty tracking, typed feedback messages, and a save path that survives session expiry. No build step, no framework, no new dependency.

**Tech Stack:** Rust 2021, axum 0.8, `include_str!` embedding, vanilla JS, vendored Pico CSS 2.1.1.

**Spec:** `docs/SPEC-web-ui.md`, plus the designer critique of the 1440/768/390 renders and the house rules of the `design-taste-frontend` skill.

## Global Constraints

- No JS build step and no npm. Everything is embedded via `src/ui/assets.rs` and `include_str!`.
- No new runtime dependency. No external font, stylesheet, or image request: the console is often offline or on a LAN.
- House rules that apply to this page: no gradients, no box-shadows on resting elements, no hairline rules, no eyebrow/pill labels above headings, no monospace outside `<code>`, no index numbering, no icon cards, no glass or backdrop blur, no emoji, no decorative shapes, one type family, no accent word inside a heading.
- Type steps: exactly four visible sizes — `1.5rem` (h1), `1.125rem` (h2/legend), `1rem` (body, labels, nav), `0.9375rem` (hints, table headers, secondary metadata). No other `font-size` declaration in the sheet.
- Do not override `--pico-font-family`: keeping Pico's stack preserves its emoji fallback. Set `--pico-font-size: 100%` in the token block so Pico's breakpoint inflation does not fire; `rem` then equals `px` at every width.
- Pico variable overrides must be declared on `:root:not([data-theme])`, `[data-theme="light"]` and `[data-theme="dark"]` — a plain `:root` loses to Pico's `:root:not([data-theme])` (0,2,0) and the accent silently does nothing.
- Every colour comes from a token declared in one of those blocks; no literal colour outside them.
- Accessibility is preserved: one `h1`, `aria-labelledby` on every section, `role="alert"` on the read-only banner, `<output aria-live="polite">` for feedback, `aria-describedby` links intact, visible focus rings, 44px minimum control height.
- Verification gate before every commit: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`. Every task that changes the render also runs:

```bash
node <skill>/scripts/lint.mjs src/ui/index.html src/ui/app.js src/ui/ui.css
node <skill>/scripts/inspect.mjs http://127.0.0.1:18090/ --allow few-pictures,no-footage,still-page,focal-point,grey-body
```

`<skill>` is `/home/j1mm0/Workspace/gitlab/skills/vendor/leonxlnx/skills/design-taste-frontend`; `--allow` takes a comma-separated list.

- Allowed rules and their one-line justification: `few-pictures` and `no-footage` — a config panel for a headless daemon has no honest photographic subject; `still-page` — decorative motion would be noise, and the only motion left is control feedback; `focal-point` — the first screen leads with the live status table, not a display headline, so a 2vw wordmark is the correct hierarchy for this page type; `grey-body` — the longest text runs are hints, which are secondary by definition, and the body and table values are set in the full ink.
- `body-size`, `measure`, and `outlined-box` must pass without an allow.
- Commit convention: Conventional Commits, subject only, <= 72 chars, no body. The Task 6 critique goes in `.superpowers/sdd/`, not in a commit message.

## Review Focus

- **Long settings form on a 1440px screen** — an operator must always know which group they are editing and be able to reach Save from anywhere in the form.
- **Unsaved edits** — a reasonable person expects to see that the form is dirty, and expects a save that hits an expired session to keep what they typed.
- **Phone at 390px** — every value must stay editable, no control may clip its own label, no hint may drop below 15px, and no machine string may force horizontal scrolling.
- **Narrow fields for short values** — a port number and a poll interval must not render in a 1,200px box.
- **Destructive versus additive versus primary controls** — Remove, Add, Logout and Save must be distinguishable at a glance. Classless Pico has no `.outline` or `.contrast` selector, so this is done with `--pico-*` variables, never with class names.
- **The stylesheet on first paint** — in token mode the first request is unauthenticated, so `/ui.css` must be a public route or the login screen renders unstyled and is never re-fetched.

---

### Task 1: Stylesheet plumbing, tokens, and page shell

**Files:**
- Create: `src/ui/ui.css`
- Modify: `src/ui/assets.rs`
- Modify: `src/ui/mod.rs:33-46` (routes), add `serve_ui_css` beside `serve_css`
- Modify: `src/ui/auth.rs:14` (`PUBLIC_ROUTES`)
- Modify: `src/ui/index.html:9-10` (link), `:13-23` (header)
- Modify: `src/ui/app.js:65-71` (`showSettings`)
- Test: `src/ui/mod.rs` `mod tests`

**Interfaces:**
- Consumes: `assets::{INDEX_HTML, APP_JS, PICO_CSS}`, `auth::PUBLIC_ROUTES: [&str; N]`
- Produces: `assets::UI_CSS`; route `/ui.css` -> `serve_ui_css()` returning `text/css`; section ids `status`, `settings`, `readonly`; tokens `--ground`, `--surface`, `--ink`, `--ink-muted`, `--accent`, `--ok`, `--danger`, `--gap`, `--radius`, `--content-width`

- [ ] **Step 1: Write the failing test**

In `index_and_healthz_return_200`, extend the URI loop and assert the status explicitly:

```rust
for uri in ["/", "/app.js", "/pico.css", "/ui.css", "/healthz"] {
    let resp = app.clone().oneshot(req(uri)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "{uri}");
}
```

Also add `"/ui.css"` to the URI list in `public_routes_need_no_auth`.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test ui::tests::index_and_healthz_return_200`
Expected: FAIL — `/ui.css` returns 401 (the auth middleware rejects it before routing), so the `assert_eq!` on status fails.

- [ ] **Step 3: Publish the route**

Add `"/ui.css"` to `PUBLIC_ROUTES` in `src/ui/auth.rs`. This is mandatory: the stylesheet is requested before login, and `app.js` never re-fetches it after authentication.

In `src/ui/mod.rs` add `.route("/ui.css", get(serve_ui_css))` after `/pico.css`, and:

```rust
async fn serve_ui_css() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        assets::UI_CSS,
    )
        .into_response()
}
```

In `src/ui/assets.rs`: `pub const UI_CSS: &str = include_str!("ui.css");`

- [ ] **Step 4: Create the stylesheet's token block and shell**

```css
:root,
:root:not([data-theme]),
[data-theme="light"],
[data-theme="dark"] {
  --surface: color-mix(in srgb, var(--pico-color) 6%, var(--pico-background-color));
  --ink: var(--pico-color);
  --ink-muted: color-mix(in srgb, var(--pico-color) 80%, var(--pico-background-color));
  --accent: #0b6fb8;
  --ok: #2f7d32;
  --danger: #b3261e;
  --gap: 24px;
  --radius: 6px;
  --content-width: 880px;
  --pico-font-size: 100%;
  --pico-primary: var(--accent);
  --pico-primary-background: var(--accent);
  --pico-primary-border: var(--accent);
  --pico-primary-hover-background: color-mix(in srgb, var(--accent) 85%, black);
  --pico-primary-hover-border: color-mix(in srgb, var(--accent) 85%, black);
  --pico-border-radius: var(--radius);
}

@media (prefers-color-scheme: dark) {
  :root:not([data-theme]) {
    --ok: #6cc36f;
    --danger: #f2b8b5;
  }
}

[data-theme="dark"] {
  --ok: #6cc36f;
  --danger: #f2b8b5;
}

.ui-main { max-width: var(--content-width); margin: 0 auto; padding: 0 24px 96px; font-size: 1rem; }
.ui-shell-header { display: flex; align-items: baseline; gap: var(--gap); padding: 32px 0 8px; flex-wrap: wrap; }
.ui-shell-header h1 { font-size: 1.5rem; margin: 0; letter-spacing: -0.01em; }
.ui-tagline { color: var(--ink-muted); margin: 4px 0 0; font-size: 1rem; }
.ui-nav { margin-left: auto; display: flex; gap: 16px; align-items: center; }
.ui-nav a { color: var(--ink); text-decoration: none; font-size: 1rem; }
.ui-nav a:hover { text-decoration: underline; }
.ui-hint, small { font-size: 0.9375rem; color: var(--ink-muted); }
section { margin-top: 48px; }
h2 { font-size: 1.125rem; font-weight: 600; letter-spacing: 0; margin-bottom: 16px; }
```

The `--ok`/`--danger` dark overrides exist because the light values sit near 2.5:1 on Pico's dark ground.

- [ ] **Step 5: Rewrite the header markup and link the sheet**

After the Pico link: `<link rel="stylesheet" href="/ui.css">`.

```html
    <header class="ui-shell-header">
      <div>
        <h1>gh-release-notify</h1>
        <p class="ui-tagline">GitHub release watch daemon &mdash; email notifications on new stable releases.</p>
      </div>
      <nav class="ui-nav" aria-label="Sections">
        <a href="#status">Status</a>
        <a href="#settings">Settings</a>
        <a href="#readonly">Read-only</a>
        <button id="logout-btn" type="button" hidden>Logout</button>
      </nav>
    </header>
```

No `class="outline"`: classless Pico has no such selector.

Rename the three section ids to match the nav: `dashboard` -> `status`, `settings-section` -> `settings`, `readonly-section` -> `readonly`. Then update all three consumers in `showSettings` (`app.js:65-71`): `el("status")`, `el("settings")`, `el("readonly")`. Missing any one throws a `TypeError` on every render and leaves the page blank.

- [ ] **Step 6: Run the gate**

Run: `cargo test ui::tests`, `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`, `lint.mjs`, `inspect.mjs`.
Expected: tests PASS; lint 0 FAIL; inspect no FAIL outside the five allowed rules.

- [ ] **Step 7: Commit**

```bash
git add src/ui/ui.css src/ui/assets.rs src/ui/mod.rs src/ui/auth.rs src/ui/index.html src/ui/app.js
git commit -m "feat: add authored stylesheet and page shell"
```

---

### Task 2: Status band and read-only section

**Files:**
- Modify: `src/ui/index.html:38-53` (status), `:146-151` (read-only)
- Modify: `src/ui/app.js:78-103` (`renderStatus`)
- Modify: `src/ui/ui.css`
- Test: `src/ui/mod.rs` `mod tests` (payload/shape assertions already cover the fields)

**Interfaces:**
- Consumes: `status.repos[].{repo,last_seen}`, `status.{last_poll_finished_at,next_poll_at}`, `readonly.state_path`, `config.ui.{bind_addr,port}`
- Produces: status rows with `<code>` machine values and `data-state="seen|none"`; one `.ui-poll-line`; read-only markup as `<dl class="ui-readonly">`

- [ ] **Step 1: Change the status markup**

```html
    <section id="status" hidden aria-labelledby="dashboard-heading">
      <h2 id="dashboard-heading">Status</h2>
      <table class="ui-status">
        <thead>
          <tr><th scope="col">Repository</th><th scope="col">Last seen release</th><th scope="col">State</th></tr>
        </thead>
        <tbody id="status-repos-body"></tbody>
      </table>
      <p class="ui-poll-line">
        <span>Last poll <time id="status-last-poll">never</time></span>
        <span>Next poll <time id="status-next-poll">unknown</time></span>
      </p>
    </section>
```

Drop the `<caption>`: Pico centres it, and the `h2` already names the section.

- [ ] **Step 2: Render machine values as `<code>`**

In `renderStatus`, build the first two cells with a `<code>` child (`document.createElement("code")`, `textContent` set to the slug and to `last_seen || "—"`), append it to the `<td>`, and set the third cell's `textContent` to `"seen"` / `"not yet"` with `td.dataset.state = r.last_seen ? "seen" : "none"`. Empty state `colSpan = 3`.

Do not style `td` with a `font-family`: the inspector treats monospace outside `<code>` as a FAIL, while `<code>` descendants are exempt.

- [ ] **Step 3: Change the read-only markup**

```html
    <section id="readonly" hidden aria-labelledby="readonly-heading">
      <h2 id="readonly-heading">Read-only values</h2>
      <dl class="ui-readonly">
        <dt>State file path</dt><dd><code id="readonly-state-path">unknown</code></dd>
        <dt>UI bind address</dt><dd><code id="readonly-bind-addr">unknown</code></dd>
        <dt>UI port</dt><dd><code id="readonly-ui-port">unknown</code></dd>
      </dl>
      <p class="ui-hint">These come from the config file and environment; changing them requires a restart.</p>
    </section>
```

- [ ] **Step 4: Style the band**

```css
.ui-status { width: 100%; font-variant-numeric: tabular-nums; }
.ui-status th { font-size: 0.9375rem; font-weight: 600; color: var(--ink-muted); text-transform: none; letter-spacing: 0; }
.ui-status td { padding: 10px 8px; border-bottom: none; color: var(--ink); }
.ui-status tbody tr:nth-child(odd) { background: var(--surface); }
.ui-status code { font-size: 1rem; background: none; padding: 0; }
.ui-status td[data-state="seen"] { color: var(--ok); }
.ui-status td[data-state="none"] { color: var(--ink-muted); }
.ui-poll-line { display: flex; gap: var(--gap); color: var(--ink-muted); font-size: 0.9375rem; margin-top: 8px; }
.ui-readonly { display: grid; grid-template-columns: max-content 1fr; gap: 8px var(--gap); }
.ui-readonly dt { color: var(--ink-muted); }
.ui-readonly dd { margin: 0; }
.ui-readonly code { word-break: break-all; background: none; padding: 0; }
```

- [ ] **Step 5: Verify**

Start the daemon on port 18090 against a temp config with three repos (hardening plan, Task 2, Step 4). Run the gate commands from Global Constraints and open the desktop and mobile screenshots.
Expected: left-aligned header row, no centred caption, slugs in monospace inside `<code>`, state colour on the third column, no horizontal overflow at 390, no `mono` or `outlined-box` FAIL.

- [ ] **Step 6: Commit**

```bash
git add src/ui/index.html src/ui/app.js src/ui/ui.css
git commit -m "feat: restyle status and read-only sections"
```

---

### Task 3: Settings grouping, control hierarchy, semantic field widths

**Files:**
- Modify: `src/ui/index.html:62-143`
- Modify: `src/ui/ui.css`
- Modify: `src/ui/app.js:18-41` (`makeRow`), `:127-132` (admin-token note copy)

**Interfaces:**
- Consumes: existing field ids and `settings-form`
- Produces: five `.ui-group` fieldsets (Polling, Email, Repositories, SMTP, Admin access) separated by a surface tone step, not a border; `.ui-row-group` rows; quiet Remove and Logout, additive Add, one filled Save

- [ ] **Step 1: Move the admin controls into their own fieldset**

Cut the `New admin token` label, input, `admin-token-note`, checkbox and `github-token-note` out of the SMTP fieldset and wrap them in:

```html
        <fieldset class="ui-group">
          <legend>Admin access</legend>
          <label for="admin-token">New admin token</label>
          <input type="password" id="admin-token" name="admin_token"
                 autocomplete="new-password" placeholder="unchanged"
                 aria-describedby="admin-token-note">
          <small id="admin-token-note" class="ui-hint">Leave empty to keep the current token.</small>
          <label for="admin-token-clear">
            <input type="checkbox" id="admin-token-clear" role="switch">
            Disable authentication (open mode)
          </label>
          <small id="github-token-note" class="ui-hint" hidden>
            A GitHub token is provided through the environment (GITHUB_TOKEN).
          </small>
        </fieldset>
```

Update the two note strings in `app.js:127-132` to match: set -> "An admin token is set. Leave empty to keep it, enter a new value to change it, or switch on disable to remove authentication." unset -> "No admin token is set: anyone who can reach this port can edit the config. Enter a value to set one."

- [ ] **Step 2: Shorten the encryption labels**

```html
          <select id="smtp-encryption" name="smtp_encryption" aria-describedby="smtp-encryption-hint">
            <option value="starttls">STARTTLS</option>
            <option value="tls">TLS (implicit)</option>
            <option value="none">None (plain)</option>
          </select>
          <small id="smtp-encryption-hint" class="ui-hint">STARTTLS upgrades a plain connection; implicit TLS is usually port 465; none is plain text, usually port 25.</small>
```

- [ ] **Step 3: Style groups by tone, width fields by meaning, rank the controls**

```css
.ui-group {
  background: var(--surface);
  border: 0;
  border-radius: var(--radius);
  padding: 8px 20px 20px;
  margin: 0 0 var(--gap);
}
.ui-group legend { font-weight: 600; font-size: 1.125rem; padding: 0 8px; }
.ui-group label { margin-top: 12px; font-size: 1rem; }
.ui-group input, .ui-group select { margin-bottom: 4px; }
#poll-interval, #smtp-port { max-width: 10ch; }
#cron-expression { max-width: 24ch; }
#sender, #sender-name, #smtp-host, #smtp-username { max-width: 42ch; }
#smtp-encryption { max-width: 28ch; }
#admin-token { max-width: 32ch; }
.ui-group .ui-hint { display: block; max-width: 62ch; margin: 4px 0 0; }
.ui-row-group { display: grid; grid-template-columns: 1fr auto; gap: 8px; align-items: start; margin-bottom: 8px; }
.ui-row-group input, .ui-row-group button { margin-bottom: 0; }

.ui-row-group button,
.ui-nav button,
#add-repo,
#add-recipient {
  --pico-background-color: transparent;
  --pico-border-color: var(--pico-muted-border-color);
  --pico-color: var(--ink);
  --pico-primary-background: transparent;
  --pico-primary-border: var(--pico-muted-border-color);
  --pico-primary-hover-background: var(--surface);
  min-height: 44px;
}
.ui-nav button:hover { --pico-color: var(--danger); --pico-border-color: var(--danger); }
.ui-row-group button:hover { --pico-color: var(--danger); --pico-border-color: var(--danger); }
#add-repo:hover, #add-recipient:hover { --pico-border-color: var(--accent); }
```

Only the Save button keeps Pico's filled primary. A border on a button is a control affordance, not a section rule, so `lint.mjs` tolerates it; the banned 1px hairline applies to dividers and to outlined boxes used as panels.

`makeRow` must also set `group.className = "ui-row-group"` and give the input an `aria-label` (`opts.placeholder`): the rows have no visible label today.

- [ ] **Step 4: Verify**

Run the gate. Open the desktop screenshot.
Expected: five groups readable as alternating tone blocks with no outlined panels; `587` about 10 characters wide; `STARTTLS` not clipped; Remove and Logout quiet, Add outlined, Save the only filled button; no `outlined-box` FAIL.

- [ ] **Step 5: Commit**

```bash
git add src/ui/index.html src/ui/app.js src/ui/ui.css
git commit -m "feat: group settings and rank form controls"
```

---

### Task 4: Save bar, dirty state, and typed feedback

**Files:**
- Modify: `src/ui/index.html:139-143` (replace the output + paragraph)
- Modify: `src/ui/app.js:48-50, 105-196, 230-268, 270-295` (`render`, `saveConfig`, `doLogin`)
- Modify: `src/ui/ui.css`

**Interfaces:**
- Consumes: `settings-message`, `save-btn`, `settings-form`
- Produces: `setMsg(node, text, kind)` with `kind` in `"ok" | "error" | "info"` writing `node.dataset.kind`; `markDirty(bool)`; `let pendingSave = false`; `saveConfig` retries after a successful re-login

- [ ] **Step 1: Change the markup**

```html
        <div class="ui-save-bar">
          <span id="unsaved" class="ui-unsaved" hidden>Unsaved changes</span>
          <output id="settings-message" aria-live="polite"></output>
          <button type="submit" id="save-btn">Save</button>
        </div>
```

Last child of the form.

- [ ] **Step 2: Style the bar without a rule**

```css
.ui-save-bar {
  position: sticky;
  bottom: 0;
  z-index: 10;
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 12px 0;
  background: var(--surface);
}
.ui-save-bar button { margin: 0; min-height: 44px; }
.ui-unsaved { color: var(--ink-muted); font-size: 0.9375rem; }
.ui-save-bar output { margin-left: auto; font-size: 0.9375rem; }
.ui-save-bar output[data-kind="ok"] { color: var(--ok); }
.ui-save-bar output[data-kind="error"] { color: var(--danger); }
.ui-save-bar output[data-kind="info"] { color: var(--ink-muted); }
```

A `border-top` here would fail `lint.mjs`; the tone step separates the bar from the form.

- [ ] **Step 3: Track dirtiness and type the messages**

```js
function setMsg(node, text, kind = "info") {
  node.textContent = text;
  node.dataset.kind = kind;
}

function markDirty(dirty) {
  document.getElementById("unsaved").hidden = !dirty;
  document.body.dataset.dirty = dirty ? "true" : "false";
}
```

Call `markDirty(false)` at the end of `render()`, `markDirty(true)` on the form's `input` and `change` events, and `markDirty(false)` after a successful save. Every failure path passes `"error"` — `doLogin` errors, save errors, both catch branches — so red always means the action did not complete.

- [ ] **Step 4: Keep edits through a 401 and re-save after login**

In `saveConfig`, replace the 401 branch with:

```js
    if (res.status === 401) {
      pendingSave = true;
      setMsg(msg, "Session expired. Sign in again: your edits are kept.", "error");
      el("login-section").hidden = false;
      return;
    }
```

In `doLogin`'s success branch, replace the unconditional `await loadConfig();` with:

```js
      if (pendingSave) {
        pendingSave = false;
        el("login-section").hidden = true;
        await saveConfig(new Event("submit"));
      } else {
        await loadConfig();
      }
```

Declare `let pendingSave = false;` next to `settingsVisible`. Nothing between the 401 and the retry touches the form, which is what preserves the edits.

- [ ] **Step 5: Clear success feedback after a moment**

After a successful save:

```js
      setMsg(msg, "Saved.", "ok");
      setTimeout(() => {
        if (msg.dataset.kind === "ok") setMsg(msg, "", "info");
      }, 4000);
```

Remove the duplicate `setMsg(msg, "Saved.")` call at `app.js:258-260`.

- [ ] **Step 6: Verify**

Type a change: "Unsaved changes" appears. Save: "Saved." in green, gone after ~4s. Restart the daemon with a token so the session is stale, then save: the form keeps its values, login appears, and the retry completes.
Expected: all three observed. Run the gate; no new FAIL.

- [ ] **Step 7: Commit**

```bash
git add src/ui/index.html src/ui/app.js src/ui/ui.css
git commit -m "feat: add sticky save bar with dirty state and typed feedback"
```

---

### Task 5: Sign-in card and the phone composition

**Files:**
- Modify: `src/ui/index.html:25-36` (login)
- Modify: `src/ui/ui.css`
- Modify: `src/ui/app.js:297-333` (`doLogout`)

**Interfaces:**
- Consumes: `login-form`, `login-token`, `login-error`, `login-hint`
- Produces: `.ui-login` card capped at `380px`; a `@media (max-width: 575px)` block that is the phone layout

- [ ] **Step 1: Style the sign-in card**

```css
.ui-login { max-width: 380px; margin: 64px 0; }
.ui-login h2 { font-size: 1.125rem; }
.ui-login button { width: 100%; min-height: 44px; }
```

Add `class="ui-login"` to the login section.

- [ ] **Step 2: Write the phone block**

```css
@media (max-width: 575px) {
  .ui-main { padding: 0 16px 80px; }
  .ui-shell-header { padding-top: 20px; }
  .ui-nav { width: 100%; margin-left: 0; justify-content: space-between; }
  .ui-row-group { grid-template-columns: 1fr; }
  .ui-row-group button { justify-self: start; }
  .ui-save-bar { flex-wrap: wrap; }
  .ui-save-bar output { margin-left: 0; flex-basis: 100%; }
  .ui-readonly { grid-template-columns: 1fr; }
  .ui-readonly dt { margin-top: 8px; }
}
```

No font-size declarations here: `--pico-font-size: 100%` already holds the root at 16px, so the type steps stay as written.

- [ ] **Step 3: Clear dirty state on logout**

Call `markDirty(false)` at the start of `doLogout`'s reset block, then keep the existing field resets.

- [ ] **Step 4: Verify at 390, 768 and 1440, including the sign-in screen**

Run the inspector twice, with separate output directories (inspect.mjs clears its `--out` directory on startup, so a second run without `--out` erases the first run's screenshots): once against the open-mode daemon (`:18090`) with `--out .taste-skill/inspect-open` for the dashboard, and once against a token-mode daemon on a second port (`ADMIN_TOKEN=review-token-12345`, port 18091) with `--out .taste-skill/inspect-token` so the sign-in card is actually photographed. Open all screenshots from both runs.
Expected: no clipped select, row groups stack, hints at 15px or more, save bar wraps to two rows at 390, read-only stacks, sign-in card at 380px on desktop, no horizontal overflow.

- [ ] **Step 5: Commit**

```bash
git add src/ui/index.html src/ui/app.js src/ui/ui.css
git commit -m "feat: add sign-in card and phone layout"
```

---

### Task 6: Gate, look, fix — two rounds maximum

**Files:** none planned; expect small edits to `src/ui/ui.css` and possibly `src/ui/app.js`

- [ ] **Step 1: Run the source lint**

```bash
node /home/j1mm0/Workspace/gitlab/skills/vendor/leonxlnx/skills/design-taste-frontend/scripts/lint.mjs src/ui/index.html src/ui/app.js src/ui/ui.css
```

Expected: `0 FAIL`. Remove a flagged device rather than adding an ignore comment.

- [ ] **Step 2: Run the browser inspector**

```bash
node /home/j1mm0/Workspace/gitlab/skills/vendor/leonxlnx/skills/design-taste-frontend/scripts/inspect.mjs \
  http://127.0.0.1:18090/ --allow few-pictures,no-footage,still-page,focal-point,grey-body
```

Expected: no FAIL outside the five allowed; `body-size`, `measure`, `outlined-box` pass at all three widths. Expect one `body-size` WARN at 768 and 1440 — the longest texts are 15px hints, which are secondary by definition and cannot be raised without breaking the four-step type scale; defend it, it does not fail the gate.

- [ ] **Step 3: Look, score, and record**

Open all six JPEGs under `.taste-skill/inspect/` and score against `references/critique.md`: hierarchy, density, alignment, grouping, scanning order, the save path, the phone composition. Write the critique to `.superpowers/sdd/web-ui-design-critique.md` (scratch, git-ignored) with the top three problems and the scores.

- [ ] **Step 4: Fix the top three, re-run, look again**

Re-run Step 2 and reopen the screenshots. Stop after two rounds; report what still falls short rather than iterating a third time.

- [ ] **Step 5: Run the code gate and commit**

```bash
cargo fmt
cargo clippy -- -D warnings
cargo clippy --all-targets -- -D warnings
cargo test
git add src/ui
git commit -m "fix: address design review of the web UI"
```

- [ ] **Step 6: Confirm dark mode**

Force `prefers-color-scheme: dark`, screenshot at 1440 and 390. Expected: ground and surface invert through the tokens; the accent button label stays at 4.5:1 or better; `--ok`/`--danger` are legible on the dark ground; no element keeps a light-only value.

- [ ] **Step 7: Update the docs**

Add one paragraph to `README.md` under the Web UI section: the three sections (Status, Settings, Read-only), the sticky save bar and dirty indicator, and that hints never carry required values.

```bash
git add README.md
git commit -m "docs: describe the web UI layout"
```
