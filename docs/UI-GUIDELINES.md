# UI design guidelines — Paper Instrument

**Status:** binding for all UI work. Read this file before changing anything under
`src/ui/`. If a change cannot follow a rule here, update this document first and
state why in the change description; do not silently diverge.

The web UI is the daemon's instrument face: a small, dense, trustworthy admin
surface for a homelabber. It is not a marketing page, a SaaS dashboard, or a
terminal costume.

## Leading words

- **Paper Instrument** — the visual direction: warm paper ground, near-black ink,
  one cobalt accent, status as the only other colour, mono for data.
- **Tick line** — the one signature element: a hairline from the last poll to the
  next poll, with a small "now" marker.
- **State surface** — a pale status pill with a glyph and a word. Never colour
  alone.
- **Save state machine** — pristine, dirty, saving, saved, invalid, error.
- **Honest state** — show only what the API can prove. No invented errors,
  presence flags, endpoints, or counts.
- **One view, one h1** — each view owns a noun `<h1>`; the product name is a
  header brand, not the page title.

## Non-negotiables

1. **No CSS framework.** One hand-written stylesheet, `src/ui/ui.css`, layered as
   `reset, tokens, base, layout, components, utilities`. No Pico, no Blades, no
   Tailwind, no CDN.
2. **No build step.** Vanilla HTML, CSS, and JavaScript only. No npm, bundler,
   preprocessor, webfont, icon font, or external asset.
3. **One accent.** Cobalt is for links, focus, and the tick marker. The primary
   button is near-black in light mode and near-white in dark mode.
4. **One view, one h1.** Status, Settings, Read-only, and Sign in each own a
   24px noun heading. The brand is a 14px/600 header span.
5. **Sentence case.** No uppercase tracked labels, including table headers and
   group titles. Nothing below 12px; inputs are 16px below 48rem.
6. **Flat surfaces.** Resting surfaces use a 1px border or one tone step. Shadows
   belong to `<dialog>` only. No gradients, glows, glass, or blur.
7. **Status is never colour alone.** Every state is glyph + word + colour, with a
   text label that survives greyscale and forced-colours mode.
8. **Honest data.** Render only fields the daemon actually exposes. A missing
   value is a grey dash; a real zero is printed; no placeholder people, fake
   growth, or invented actions.
9. **Explicit save.** One atomic save per settings view, submit-first validation,
   persistent status, no autosave, no toasts.
10. **Keyboard and screen reader first.** Visible focus, 44px touch targets on
    coarse pointers, 320px reflow, labelled controls, and a live region that
    announces only meaningful state changes.

## Visual language

### Colour roles

Exact values live in `src/ui/ui.css` under `@layer tokens`. Roles are fixed:

| Role | Token | Use |
|---|---|---|
| Canvas | `--canvas` | Page ground |
| Surface | `--surface` | Panels, table, inputs |
| Raised surface | `--surface-2` | Save bar, active nav pill, hover |
| Ink | `--ink` | Primary text, identity, key values |
| Muted ink | `--ink-muted` | Secondary text, labels, meta |
| Subtle ink | `--ink-subtle` | Placeholders, timestamps |
| Border | `--border` | Row rules, decorative separators |
| Strong border | `--border-strong` | Control edges, save-bar edge (3:1) |
| Accent | `--accent` | Links, focus, tick marker |
| Primary button | `--btn-bg`, `--btn-fg` | Near-black/near-white fill |
| Positive | `--ok`, `--ok-bg` | Watching, saved |
| Warning | `--warn`, `--warn-bg` | Read-only, unsaved hint |
| Danger | `--danger`, `--danger-bg` | Errors, destructive confirmation |
| Neutral | `--neutral`, `--neutral-bg` | No release yet, neutral pills |

Rules:

- Text contrast is at least 4.5:1; UI boundaries and focus are at least 3:1.
- Status pills use a pale tint with dark text in light mode, and a deep tint with
  light text in dark mode.
- A dark or saturated sidebar, a navy/slate dark theme, violet/indigo accents,
  and cream-with-terracotta are outside this direction.
- Dark mode follows `prefers-color-scheme`; there is no manual theme toggle.

### Type

| Role | Size / line | Weight | Family |
|---|---|---|---|
| View `h1` | 24px / 1.2 | 600 | sans |
| Section `h2` | 16px / 1.3 | 600 | sans |
| Body / UI | 14px / 1.45 | 400 | sans |
| Data | 13px / 1.4 | 400 | mono, tabular |
| Table head | 12px / 1.35 | 500 | sans |
| Hint / error / pill | 12px / 1.45 | 400 / 500 | sans |
| Time | 12px / 1.2 | 400 | mono, tabular |

- System sans for UI, `ui-monospace` for repo slugs, release tags, cron, paths,
  and times. Tabular figures wherever numbers align.
- Headings use `letter-spacing: -0.01em`; small labels use `0`.
- Do not introduce a display face or a second sans. A mono for data is the only
  second family.

### Space, shape, elevation, motion

- Spacing follows a 4px base (`--sp-1` … `--sp-12`); use one gutter:
  `clamp(1rem, 3vw, 2rem)`.
- Radii: **8px cards/groups, 6px controls, 4px chips**, `999px` pills.
- Page width: 68rem. Header: 48px. Desktop controls: 36px; coarse pointers: 44px.
  Table rows: 44px.
- Elevation: 1px border or one tone step. `<dialog>` is the only shadow.
- Motion is feedback only: 120ms hover/focus, 180ms bar/banner change. No
  entrance animations. All motion is disabled under
  `prefers-reduced-motion: reduce`.
- Focus is `outline: 2px solid var(--focus); outline-offset: 1px`. Never remove
  the outline without an equivalent visible replacement.

## Shell and navigation

- **Desktop (≥48rem):** 48px header; brand left; Status, Settings, Read-only,
  and Log out right. Active item is a neutral `--surface-2` pill with ink at 600
  and `aria-current="page"`. No accent underline.
- **Mobile (<48rem):** 48px header with brand and a Log out icon button; fixed
  56px bottom bar with Status, Settings, and Read-only (20px inline SVG + 12px
  label). Log out stays in the header.
- The three-destination bottom bar is a deliberate deviation from the four-to-five
  destinations common in admin apps: this product has exactly three views.
- Main content is centered at 68rem with one gutter. Each view owns its `h1`.
- Use hash routes (`#status`, `#settings`, `#readonly`) so back/forward and deep
  links work.
- One global banner slot sits below the header. Priority: read-only config,
  unsaved changes outside Settings, then save/fetch error. Never stack banners;
  never auto-dismiss an error.

## Views

### Status

- `h1` → tick line → table.
- Desktop table: fixed layout, columns 46% / 34% / 20%; 12px sentence-case
  header; 44px rows; 1px bottom rules; no zebra, no header fill band.
- Repo cell: mono 13px; `owner/` muted, repo ink, one-line ellipsis with the full
  value in `title`.
- Release cell: mono 13px tag; `—` when never seen.
- State cell: 24px pale pill, 12px/500, 12px glyph + word. Exactly two states:
  **Watching** (`last_seen` present) and **No release yet** (`last_seen` absent).
  The snapshot cannot distinguish "never polled" from "polled with no stable
  release", so do not imply either.
- Mobile (≤640px): render a real two-line list from the same data. Line 1: repo
  slug + state pill; line 2: release tag. Do not restyle table elements with
  `display` changes; table semantics must survive.
- Empty state: one sentence and one action in the frame — "No repositories yet."
  + "Add repository" (opens Settings and focuses the repository field).
- Caption for screen readers: "Repositories and their last seen release."

### Tick line

```
last 14:02:11Z ──────────▪──────────────── next 14:12:11Z
```

- Two `<time datetime>` values in 12px mono with tabular figures, muted ink.
- 1px rule: elapsed segment muted ink, remaining segment border tone.
- 6×6 accent marker at `(now − last) / (next − last)`, clamped 2–98%.
- The rule and marker are `aria-hidden`; the timestamps are the accessible
  content. Do not make the tick line a live region and do not announce a
  countdown.
- Fallback: if `last` is absent (first snapshot), or `next` is absent/past/≤ last,
  render a dashed 1px rule, omit the marker, and let the text carry the times
  (`last never · next 14:12Z`).
- Place it directly under the Status `h1`, above the table.

### Settings

- `h1` → form. Five static groups: Polling, Repositories, Email, SMTP, Admin
  access. All visible at every width; no accordion for the five core groups.
  `<details>` is reserved for genuinely advanced fields added later.
- Group: 1px border, 8px radius, no shadow; title 16/600; content padding
  16–20px; 24px between groups.
- Labels above inputs at 14/500; hints below at 12px muted; 16px between fields.
- Field grid: one column below 48rem; `repeat(auto-fit, minmax(15rem, 1fr))`
  above, inside a container query. Short pairs may share a row; long fields span.
- Dynamic recipient/repo rows: input + 44px remove button; "Add recipient" /
  "Add repository" is a secondary/dashed full-width button. No confirm dialog on
  remove: nothing is applied until Save. Focus the new row's first input on add;
  return focus sensibly on remove.
- **Admin token:** password field, `autocomplete="new-password"`, show/hide
  button; show `Set` / `Not set` from `config.ui.admin_token_set`. Blank keeps the
  current token. Keep the "Disable authentication (open mode)" switch — the API
  clears the token by submitting `admin_token: ""` — and word the consequence
  plainly: anyone who can reach the port can edit the config. State that an
  `ADMIN_TOKEN` environment value overrides the file and cannot be detected by
  the UI.
- **SMTP password:** password field, `autocomplete="new-password"`; blank keeps
  the existing password. Clearing is unsupported. The API exposes no
  `smtp_password_set`, so render no Set/Not set pill. When
  `env_managed.smtp_password` is true, show "Managed by the SMTP_PASSWORD
  environment variable" and render the field `readonly`.
- Env-managed values are `readonly`, not `disabled`, with a hint naming the
  environment variable.
- Save bar: `--surface-2`, 1px `--border-strong` top edge, no shadow; status
  left, actions right. It is sticky while dirty, saving, saved, invalid, or in
  error, and static at the form's end while pristine. Below 36rem the status
  takes its own line above full-width buttons. Respect
  `env(safe-area-inset-bottom)`; reserve form bottom space and set
  `scroll-padding-bottom` so a focused control is never hidden.

### Read-only

- `h1` → `<dl>` grid (`minmax(10rem, 14rem) 1fr`), 1px row rules.
- `dt` 12px muted; `dd` 13px mono; long paths wrap anywhere.
- Rows: state path, UI bind address, UI port, auth mode, GitHub token ("from
  GITHUB_TOKEN" / "not set"). Show SMTP password only as "from SMTP_PASSWORD"
  when `env_managed.smtp_password` is true; the API does not expose whether a
  config-file password exists. Never render secret values.

### Sign in

- `h1` "Sign in"; one visible "Admin token" field, `type="password"`,
  `autocomplete="current-password"`, `autocapitalize="none"`,
  `spellcheck="false"`; paste allowed; show/hide button.
- Failure: generic "That token is not correct.", focus the field, `role="alert"`.
  Do not reveal whether a token exists.
- 429: "Too many attempts. Try again later." The server sends no `Retry-After`;
  do not show a countdown unless the API adds one.
- On success, focus the view `h1` (`tabindex="-1"`).

## Components

- **Buttons:** primary (near-black/near-white), secondary (surface + strong
  border), ghost (text), destructive (danger text/border, only for destructive
  confirmation). 36px desktop, 44px coarse. Sentence case. One primary per view.
- **Fields:** label above; hint below; error replaces/augments the hint with
  `aria-invalid` and `aria-describedby`. Native `<select>` appearance and native
  pickers; style only border, height, and font.
- **Switch:** native checkbox with `appearance: none`; 44×24 track, 20px thumb;
  on-track is ink, not accent; adjacent text says Enabled/Disabled.
- **Status pill:** 24px tall, 12/500, 12px inline-SVG glyph + word, pill radius,
  pale tint/dark text. One status mark per row.
- **Banner:** icon + one sentence + one action link; `role="status"` for info,
  `role="alert"` for failures; no stacking, no auto-dismiss for errors.
- **Save bar:** six states below. One persistent `role="status"` region, never a
  toast.
- **Dialog:** native `<dialog>` for the unsaved-logout confirmation. Focus moves
  into it, Esc closes, focus returns to the trigger.
- **Icons:** inline SVG only, one outline set, ~1.5px stroke, `currentColor`,
  accessible names. No icon font, no remote sprites.

### Save state machine

| State | Status text | Save |
|---|---|---|
| Pristine | "All changes saved" + check | Enabled unless `config_writable` is false; secondary weight; click says "No changes to save." |
| Dirty | "Unsaved changes" + amber dot and words | Enabled, primary |
| Saving | "Saving…" + spinner; `aria-busy`; double submit blocked | `aria-disabled="true"` with a JS guard; focus stays |
| Saved | "Saved 14:12" + check; reverts to pristine after ~5s | Secondary; no re-announcement |
| Invalid | "N fields need attention" + Review | Primary |
| Error | "Couldn't save" + alert glyph + Try again | Primary; banner carries the server message |

- Dirty baseline is the normalized semantic model after load/save (trim, empty
  cron → null, drop blank rows, coerce numbers). Hydration must not mark dirty.
  Reverting to the baseline clears dirty.
- Register `beforeunload` only while dirty; remove it when clean.
- Logging out while dirty opens the confirmation dialog; clear the flag before
  navigating on confirm.
- Success auto-clears after ~5s without re-announcing; errors persist until the
  next save attempt.

## Validation and errors

- `<form novalidate>`; validate on submit, not on blur or first keystroke.
- Client checks only for stable constraints: required, `type=email`, number
  min/max, port 1–65535, poll interval ≥60, `owner/repo` shape, duplicate repos,
  non-empty recipients. Do not reimplement cron parsing in JavaScript; the server
  is authoritative for cron and cross-field rules.
- On client failure: inline messages plus a summary at the top of the form
  ("N fields need attention") with ordered links; focus the summary. Announce the
  count in the status region. Do not put `role="alert"` on both the summary and
  the status region.
- Server validation failures return 400/409/500 `{"error": string}` with no field
  structure. Show the string in the banner and save-bar status, keep values and
  dirty state, and do not guess a field mapping.
- A 401 during save means the session expired: show Sign in, keep all values and
  the dirty state, and resume the pending save after re-authentication.
- Clear a field's error on input after a failed submit; do not re-validate the
  whole form.

## Honest-state contract

The UI may rely only on these API facts:

| Fact | Source | Consequence |
|---|---|---|
| Per-repo state | `StatusSnapshot.repos[].last_seen` | Only Watching / No release yet |
| Poll times | `last_poll_finished_at`, `next_poll_at` | Tick line, with fallback for missing values |
| Admin token presence | `config.ui.admin_token_set` | Set/Not set pill |
| SMTP password env | `env_managed.smtp_password` | Env-managed note only; no presence pill |
| GitHub token env | `env_managed.github_token` | Read-only row |
| Config writability | `config_writable` | Read-only banner; Save disabled only here |
| Save errors | 400/409/500 `{"error": string}` | Banner + status; no field mapping |
| Auth expiry | 401 | Sign in, preserve edits, resume save |
| Login rate limit | 429 `{"error":"too many attempts"}` | Static retry message, no countdown |

Do not add controls or states the daemon cannot honour: no check-now, send-test
email, clear-SMTP-password, per-repo error pills, secret values, or retry
countdown.

## Responsive rules

| Width | Behaviour |
|---|---|
| < 36rem | One column; bottom nav; 16px fields; 44px controls; 16px gutter; save bar stacks status above buttons |
| 36–47.99rem | One column; small field pairs may go 2-up; 20px gutter |
| 48–63.99rem | Header nav replaces bottom bar; 2-column field grid; 24px h1 |
| ≥ 64rem | 68rem centered canvas; 32px gutter; 2–3 column field grid |

- Test at 320, 390, 768, 1024, and 1440px. No horizontal page scrolling at 320px.
- The table becomes a real two-line list below 640px; fields never drop, they
  move to line 2.
- Touch targets are at least 44×44px on coarse pointers, with 8px between
  adjacent targets.

## Copy rules

- Sentence case everywhere; no exclamation marks; no emoji.
- Name the thing: "Add repository", "Save changes", "No release yet", not
  "Submit" or "Item".
- Errors say what happened and how to fix it: "Enter a poll interval of at least
  60 seconds."
- Never claim a save, poll, or test that the daemon did not perform.
- Empty states say what is missing and offer the one next action.

## Implementation map

| File | Responsibility |
|---|---|
| `src/ui/index.html` | Semantic structure, views, forms, dialogs, inline SVG icons |
| `src/ui/ui.css` | Reset, tokens, base, layout, components, responsive, environment rules |
| `src/ui/app.js` | Hash routing, render, dirty tracking, validation, save/login/logout, tick line |
| `src/ui/assets.rs` | `include_str!` constants |
| `src/ui/mod.rs` | Routes, handlers, auth middleware, tests |

- Token values are canonical in `src/ui/ui.css`; this document fixes their roles
  and usage. If a role or value changes, update both in the same change.
- Keep user data out of `innerHTML`; use `textContent` and DOM APIs.
- Preserve the CSRF header on every non-GET request and never log or echo
  secrets.

## Verification gate

Run before claiming UI work is done:

```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
node --check src/ui/app.js
```

Then open the UI and check at 1440, 768, 390, and 320px:

- [ ] Each view has exactly one `h1` and a heading on every section.
- [ ] No horizontal page scroll at 320px; the table becomes a two-line list.
- [ ] Tab order matches visual order; every control has a visible focus ring.
- [ ] Status is glyph + word + colour; greyscale still reads.
- [ ] Empty, save-success, save-error, validation-error, 401, and login-error
      states were seen.
- [ ] The tick line handles both timestamps, missing `last`, and missing/past
      `next`.
- [ ] Save is enabled when clean unless `config_writable` is false; dirty state
      survives navigation within the app; `beforeunload` fires only while dirty.
- [ ] No secret value appears in the DOM, a response, or the console.
- [ ] `rg -in "pico" src/ui/index.html src/ui/ui.css src/ui/app.js` returns
      nothing.
- [ ] The stylesheet contains no gradients, glows, glass, resting shadows, or
      uppercase tracked labels.
- [ ] Contrast passes 4.5:1 text and 3:1 UI/focus in both light and dark mode.
- [ ] Lighthouse accessibility is 100 on Status and pristine Settings.
