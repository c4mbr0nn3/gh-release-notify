"use strict";

const CSRF_VALUE = "gh-release-notify";
const REFRESH_MS = 15000;
const SAVED_TTL_MS = 5000;
const NOTE_TTL_MS = 4000;

const el = (id) => document.getElementById(id);

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
const REPO_RE = /^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/;

const ICONS = {
  check: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>',
  clock: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>',
  alert: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h16.9a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"/><path d="M12 9v4"/><path d="M12 17h.01"/></svg>',
  info: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 8h.01"/><path d="M12 11v5"/></svg>',
  lock: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="11" width="16" height="9" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/></svg>',
};

const VIEWS = ["status", "settings", "readonly", "login"];
const HEADINGS = {
  status: "status-heading",
  settings: "settings-heading",
  readonly: "readonly-heading",
  login: "login-heading",
};

let authed = false;
let booted = false;
let authMode = "token";
let configWritable = true;
let envManaged = { smtp_password: false, github_token: false };
let adminTokenSet = false;
let lastSnapshot = null;

let baseline = null;
let dirty = false;
let unloadBound = false;

let saving = false;
let loginBusy = false;
let pendingSave = false;
let saveUi = { state: "pristine", note: null, count: 0, savedAt: null };
let saveTimer = null;
let noteTimer = null;

let bannerError = null;
let sessionNotice = null;
let currentView = null;
const validationMap = new Map();
let rowSeq = 0;

function icon(name) {
  const template = document.createElement("template");
  template.innerHTML = ICONS[name];
  const svg = template.content.firstElementChild.cloneNode(true);
  svg.setAttribute("aria-hidden", "true");
  return svg;
}

function utcClock(ms) {
  return new Date(ms).toISOString().slice(11, 19) + "Z";
}

function utcMinutes(ms) {
  return new Date(ms).toISOString().slice(11, 16) + "Z";
}

function setDateTime(node, iso) {
  if (iso) {
    node.dateTime = iso;
    node.title = iso;
  } else {
    node.removeAttribute("datetime");
    node.removeAttribute("title");
  }
}

function sameModel(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

const toNumberOrNull = (value) => {
  const trimmed = value.trim();
  if (trimmed === "") return null;
  const n = Number(trimmed);
  return Number.isFinite(n) ? n : null;
};

function listValues(id) {
  return [...el(id).querySelectorAll("input")]
    .map((input) => input.value.trim())
    .filter((value) => value.length > 0);
}

function semanticModel() {
  return {
    poll_interval_seconds: toNumberOrNull(el("poll-interval").value),
    cron_expression: el("cron-expression").value.trim() || null,
    sender: el("sender").value.trim(),
    sender_name: el("sender-name").value.trim() || null,
    repos: listValues("repos-list"),
    recipients: listValues("recipients-list"),
    smtp_host: el("smtp-host").value.trim(),
    smtp_port: toNumberOrNull(el("smtp-port").value),
    smtp_encryption: el("smtp-encryption").value,
    smtp_username: el("smtp-username").value.trim(),
  };
}

function secretsChanged() {
  return (
    el("admin-token").value !== "" ||
    el("smtp-password").value !== "" ||
    el("admin-disable-auth").checked
  );
}

const onBeforeUnload = (event) => {
  event.preventDefault();
  event.returnValue = "";
};

function updateBeforeUnload() {
  if (dirty && !unloadBound) {
    window.addEventListener("beforeunload", onBeforeUnload);
    unloadBound = true;
  } else if (!dirty && unloadBound) {
    window.removeEventListener("beforeunload", onBeforeUnload);
    unloadBound = false;
  }
}

function refreshDirty() {
  const next =
    baseline === null ||
    !sameModel(semanticModel(), baseline) ||
    secretsChanged();
  if (next === dirty) {
    if (saveUi.state === "pristine" || saveUi.state === "dirty") renderSaveBar();
    updateBeforeUnload();
    return;
  }
  dirty = next;
  if (!dirty) {
    clearTimeout(saveTimer);
    clearTimeout(noteTimer);
    if (saveUi.state !== "saved") saveUi = { state: "pristine", note: null, count: 0, savedAt: null };
  } else if (saveUi.state === "saved" || saveUi.state === "pristine") {
    clearTimeout(saveTimer);
    saveUi = { state: "pristine", note: null, count: 0, savedAt: null };
  }
  renderSaveBar();
  updateBeforeUnload();
  renderBanner();
}

/* ----------------------------------------------------------------- views */

function setView(name, opts = {}) {
  const focus = opts.focus !== false;
  if (!VIEWS.includes(name)) name = "status";
  if (!authed && name !== "login") name = "login";
  if (authed && name === "login") name = "status";
  const changed = name !== currentView;
  currentView = name;
  el("main-nav").hidden = !authed;
  el("bottom-nav").hidden = !authed;
  for (const view of VIEWS) el(`view-${view}`).hidden = view !== name;
  for (const link of document.querySelectorAll("[data-nav]")) {
    if (link.dataset.nav === name) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  }
  if (name === "status" && lastSnapshot) renderStatusView(lastSnapshot);
  renderBanner();
  if (changed && focus) el(HEADINGS[name]).focus();
}

function routeTarget() {
  const name = (location.hash || "").replace(/^#/, "");
  return VIEWS.includes(name) ? name : "status";
}

function applyRoute(opts) {
  setView(routeTarget(), opts);
}

window.addEventListener("hashchange", () => {
  applyRoute({ focus: true });
});

/* ---------------------------------------------------------------- banner */

function renderBanner() {
  const slot = el("banner-slot");
  slot.replaceChildren();
  let banner = null;
  if (authed && !configWritable) {
    banner = { cls: "banner-warn", role: "status", icon: "lock", text: "The configuration file is not writable. Saving is disabled." };
  } else if (authed && dirty && currentView !== "settings") {
    banner = { cls: "banner-warn", role: "status", icon: "info", text: "You have unsaved changes in Settings.", action: { label: "Open Settings", href: "#settings" } };
  } else if (bannerError) {
    banner = { cls: "banner-danger", role: "alert", icon: "alert", text: bannerError };
  } else if (sessionNotice) {
    banner = { cls: "banner-info", role: "status", icon: "info", text: sessionNotice };
  }
  if (!banner) return;
  const node = document.createElement("div");
  node.className = `banner ${banner.cls}`;
  node.setAttribute("role", banner.role);
  node.appendChild(icon(banner.icon));
  const text = document.createElement("p");
  text.textContent = banner.text;
  node.appendChild(text);
  if (banner.action) {
    const link = document.createElement("a");
    link.href = banner.action.href;
    link.textContent = banner.action.label;
    node.appendChild(link);
  }
  slot.appendChild(node);
}

/* ----------------------------------------------------------------- status */

function makeSlug(slug) {
  const code = document.createElement("code");
  code.className = "repo-slug";
  code.title = slug;
  const cut = slug.indexOf("/");
  if (cut > 0) {
    const owner = document.createElement("span");
    owner.className = "repo-owner";
    owner.textContent = slug.slice(0, cut + 1);
    code.appendChild(owner);
  }
  const name = document.createElement("span");
  name.className = "repo-name";
  name.textContent = cut > 0 ? slug.slice(cut + 1) : slug;
  code.appendChild(name);
  return code;
}

function makeStatePill(seen) {
  const pill = document.createElement("span");
  pill.className = `pill ${seen ? "pill-ok" : "pill-neutral"}`;
  pill.appendChild(icon(seen ? "check" : "clock"));
  const word = document.createElement("span");
  word.textContent = seen ? "Watching" : "No release yet";
  pill.appendChild(word);
  return pill;
}

function makeReleaseCell(lastSeen) {
  const code = document.createElement("code");
  code.className = "release-code";
  if (lastSeen) {
    code.textContent = lastSeen;
  } else {
    code.className = "release-code muted-dash";
    code.textContent = "\u2014";
  }
  return code;
}

function statusRow(entry) {
  const seen = entry.last_seen != null;
  const tr = document.createElement("tr");
  const repoCell = document.createElement("td");
  repoCell.appendChild(makeSlug(entry.repo));
  const releaseCell = document.createElement("td");
  releaseCell.appendChild(makeReleaseCell(entry.last_seen));
  const stateCell = document.createElement("td");
  stateCell.appendChild(makeStatePill(seen));
  tr.appendChild(repoCell);
  tr.appendChild(releaseCell);
  tr.appendChild(stateCell);
  return tr;
}

function statusListItem(entry) {
  const seen = entry.last_seen != null;
  const li = document.createElement("li");
  const line1 = document.createElement("div");
  line1.className = "repo-line";
  line1.appendChild(makeSlug(entry.repo));
  line1.appendChild(makeStatePill(seen));
  const line2 = document.createElement("p");
  line2.className = "release-line";
  const label = document.createElement("span");
  label.className = "sr-only";
  label.textContent = "Last seen release: ";
  line2.appendChild(label);
  line2.appendChild(makeReleaseCell(entry.last_seen));
  li.appendChild(line1);
  li.appendChild(line2);
  return li;
}

function renderStatusView(snapshot) {
  const repos = snapshot && Array.isArray(snapshot.repos) ? snapshot.repos : [];
  el("view-status").dataset.empty = repos.length === 0 ? "true" : "false";
  const body = el("status-body");
  body.replaceChildren();
  const list = el("status-list");
  list.replaceChildren();
  for (const entry of repos) {
    body.appendChild(statusRow(entry));
    list.appendChild(statusListItem(entry));
  }
  renderTickLine(snapshot);
}

function renderTickLine(snapshot) {
  const lastIso = (snapshot && snapshot.last_poll_finished_at) || null;
  const nextIso = (snapshot && snapshot.next_poll_at) || null;
  const lastMs = lastIso ? Date.parse(lastIso) : NaN;
  const nextMs = nextIso ? Date.parse(nextIso) : NaN;
  const valid =
    Number.isFinite(lastMs) && Number.isFinite(nextMs) && nextMs > lastMs;
  const lastNode = el("tick-last");
  const nextNode = el("tick-next");
  lastNode.textContent = Number.isFinite(lastMs) ? utcClock(lastMs) : "never";
  setDateTime(lastNode, Number.isFinite(lastMs) ? lastIso : null);
  nextNode.textContent = Number.isFinite(nextMs) ? utcClock(nextMs) : "\u2014";
  setDateTime(nextNode, Number.isFinite(nextMs) ? nextIso : null);
  const root = el("tick-line");
  root.classList.toggle("tick-empty", !valid);
  if (valid) {
    const fraction = Math.min(Math.max((Date.now() - lastMs) / (nextMs - lastMs), 0), 1);
    const pct = (2 + fraction * 96).toFixed(2) + "%";
    el("tick-rule").style.setProperty("--tick-frac", pct);
  }
}

async function refreshStatus() {
  if (document.hidden || !authed) return;
  try {
    const res = await fetch("/api/config");
    if (res.status === 401) {
      enterLogin("Your session has ended. Sign in again.");
      return;
    }
    if (!res.ok) return;
    const data = await res.json();
    if (data.status) {
      lastSnapshot = data.status;
      if (currentView === "status") renderStatusView(data.status);
    }
  } catch (err) {
    void err;
  }
}

/* --------------------------------------------------------------- settings */

function makeRow(container, opts) {
  rowSeq += 1;
  const inputId = `${container.id}-input-${rowSeq}`;
  const row = document.createElement("div");
  row.className = "row";
  row.setAttribute("role", "listitem");
  const input = document.createElement("input");
  input.type = opts.type;
  input.id = inputId;
  input.value = opts.value || "";
  input.placeholder = opts.placeholder;
  input.setAttribute("aria-label", opts.ariaLabel);
  input.spellcheck = false;
  if (opts.type === "email") input.autocapitalize = "none";
  const errNode = document.createElement("p");
  errNode.className = "field-error";
  errNode.id = `${inputId}-error`;
  errNode.hidden = true;
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "btn small";
  remove.textContent = "Remove";
  remove.setAttribute("aria-label", `Remove ${opts.value || "this row"}`);
  row.appendChild(input);
  row.appendChild(errNode);
  row.appendChild(remove);
  container.appendChild(row);
  input.addEventListener("input", () => {
    remove.setAttribute("aria-label", `Remove ${input.value.trim() || "this row"}`);
  });
  remove.addEventListener("click", () => {
    if (validationMap.has(inputId)) clearEntry(inputId);
    const rows = [...container.children];
    const index = rows.indexOf(row);
    row.remove();
    const remaining = [...container.children];
    const neighbour = remaining[index] || remaining[index - 1];
    if (neighbour) {
      const neighbourInput = neighbour.querySelector("input");
      if (neighbourInput) neighbourInput.focus();
    } else {
      el(opts.addButtonId).focus();
    }
    refreshDirty();
  });
  return input;
}

function buildRows(container, opts) {
  container.replaceChildren();
  const values = opts.values.length > 0 ? opts.values : [""];
  for (const value of values) {
    makeRow(container, { ...opts, value });
  }
}

function addRow(containerId, opts) {
  const input = makeRow(el(containerId), { ...opts, value: "" });
  input.focus();
  refreshDirty();
}

function syncAdminState() {
  const tokenInput = el("admin-token");
  const disable = el("admin-disable-auth");
  const hint = el("admin-token-hint");
  const state = el("admin-disable-state");
  if (authMode === "proxy") {
    tokenInput.disabled = true;
    disable.disabled = true;
    disable.checked = false;
    state.textContent = "Proxy";
    hint.textContent =
      "Proxy authentication is active (trust_proxy_auth). Admin token settings are managed by the proxy configuration.";
    return;
  }
  disable.disabled = false;
  tokenInput.disabled = disable.checked;
  state.textContent = disable.checked ? "Enabled" : "Disabled";
  if (disable.checked) {
    hint.textContent =
      "Anyone who can reach this port could read status and edit the configuration.";
  } else if (adminTokenSet) {
    hint.textContent =
      "An admin token is set. Leave empty to keep it, or enter a new value to change it.";
  } else {
    hint.textContent =
      "No admin token is set: anyone who can reach this port can edit the configuration. Enter a value to enable authentication.";
  }
}

function syncSmtpPasswordState() {
  const input = el("smtp-password");
  const hint = el("smtp-password-hint");
  if (envManaged.smtp_password) {
    input.readOnly = true;
    input.value = "";
    hint.textContent = "Managed by the SMTP_PASSWORD environment variable.";
  } else {
    input.readOnly = false;
    hint.textContent = "Leave empty to keep the existing password. Clearing is not supported.";
  }
}

function renderAdminTokenPill() {
  const pill = el("admin-token-state");
  pill.textContent = adminTokenSet ? "Set" : "Not set";
  pill.className = `pill ${adminTokenSet ? "pill-ok" : "pill-neutral"}`;
}

function renderReadonly(data) {
  const config = data.config || {};
  const ui = config.ui || {};
  el("readonly-state-path").textContent =
    (data.readonly && data.readonly.state_path) || "\u2014";
  el("readonly-bind-addr").textContent = ui.bind_addr || "\u2014";
  el("readonly-ui-port").textContent = ui.port != null ? ui.port : "\u2014";
  el("readonly-auth-mode").textContent =
    ({ token: "Token", proxy: "Proxy", open: "Open" })[data.auth_mode] ||
    data.auth_mode ||
    "\u2014";
  el("readonly-github-token").textContent = envManaged.github_token
    ? "from GITHUB_TOKEN"
    : "not set";
  el("readonly-smtp-row").hidden = !envManaged.smtp_password;
}

function resetStatusView() {
  renderStatusView({ repos: [], last_poll_finished_at: null, next_poll_at: null });
}

function resetSettingsForm() {
  el("poll-interval").value = "";
  el("cron-expression").value = "";
  el("sender").value = "";
  el("sender-name").value = "";
  el("smtp-host").value = "";
  el("smtp-port").value = "";
  el("smtp-encryption").value = "starttls";
  el("smtp-username").value = "";
  el("smtp-password").value = "";
  el("admin-token").value = "";
  el("admin-disable-auth").checked = false;
  syncAdminState();
  syncSmtpPasswordState();
}

function collectEdit() {
  const edit = {
    poll_interval_seconds: Number(el("poll-interval").value),
    cron_expression: el("cron-expression").value.trim() || null,
    sender: el("sender").value.trim(),
    sender_name: el("sender-name").value.trim() || null,
    repos: listValues("repos-list"),
    recipients: listValues("recipients-list"),
    smtp_host: el("smtp-host").value.trim(),
    smtp_port: Number(el("smtp-port").value),
    smtp_encryption: el("smtp-encryption").value,
    smtp_username: el("smtp-username").value.trim(),
  };
  if (el("admin-disable-auth").checked) {
    edit.admin_token = "";
  } else if (el("admin-token").value !== "") {
    edit.admin_token = el("admin-token").value;
  }
  const password = el("smtp-password").value;
  if (password !== "") edit.smtp_password = password;
  return edit;
}

/* ------------------------------------------------------------- validation */

function setFieldError(inputId, message) {
  const input = el(inputId);
  input.setAttribute("aria-invalid", "true");
  const errNode = el(`${inputId}-error`);
  errNode.textContent = message;
  errNode.hidden = false;
  validationMap.set(inputId, {
    message,
    focusId: inputId,
    errId: `${inputId}-error`,
  });
}

function setGroupError(listId, errId, message, focusId) {
  const errNode = el(errId);
  errNode.textContent = message;
  errNode.hidden = false;
  validationMap.set(listId, { message, focusId, errId });
}

function clearEntry(key) {
  const entry = validationMap.get(key);
  if (!entry) return;
  validationMap.delete(key);
  const errNode = el(entry.errId);
  if (errNode) {
    errNode.hidden = true;
    errNode.textContent = "";
  }
  const focusEl = el(entry.focusId);
  if (focusEl) focusEl.removeAttribute("aria-invalid");
  renderErrorSummary();
  if (validationMap.size === 0 && saveUi.state === "invalid") {
    saveUi = { state: "pristine", note: null, count: 0, savedAt: null };
    refreshDirty();
  }
}

function clearValidationUi() {
  for (const key of [...validationMap.keys()]) clearEntry(key);
  el("error-summary").hidden = true;
}

function renderErrorSummary() {
  const summary = el("error-summary");
  const title = el("error-summary-title");
  const list = el("error-summary-list");
  if (validationMap.size === 0) {
    summary.hidden = true;
    return;
  }
  summary.hidden = false;
  const count = validationMap.size;
  title.textContent = `${count} ${count === 1 ? "field needs" : "fields need"} attention`;
  list.replaceChildren();
  for (const [, entry] of validationMap) {
    const li = document.createElement("li");
    const link = document.createElement("a");
    link.href = "#";
    link.textContent = entry.message;
    link.addEventListener("click", (event) => {
      event.preventDefault();
      const focusEl = el(entry.focusId);
      if (focusEl) focusEl.focus();
    });
    li.appendChild(link);
    list.appendChild(li);
  }
}

function validateRowList(listId, errId, kind, emptyMessage, addButtonId) {
  const container = el(listId);
  for (const row of container.children) {
    const input = row.querySelector("input");
    if (input) {
      input.removeAttribute("aria-invalid");
      const errNode = el(`${input.id}-error`);
      errNode.hidden = true;
      errNode.textContent = "";
    }
  }
  for (const key of [...validationMap.keys()]) {
    if (key.startsWith(`${listId}-input-`)) validationMap.delete(key);
  }
  const seen = new Set();
  let hasValue = false;
  for (const row of container.children) {
    const input = row.querySelector("input");
    if (!input) continue;
    const value = input.value.trim();
    if (value === "") continue;
    hasValue = true;
    let message = null;
    if (kind === "repo") {
      if (!REPO_RE.test(value)) message = "Use the format owner/repo.";
      else if (seen.has(value.toLowerCase())) message = "This repository is already listed.";
      else seen.add(value.toLowerCase());
    } else if (!EMAIL_RE.test(value)) {
      message = "Enter a valid email address.";
    }
    if (message) {
      input.setAttribute("aria-invalid", "true");
      const errNode = el(`${input.id}-error`);
      errNode.textContent = message;
      errNode.hidden = false;
      validationMap.set(input.id, {
        message,
        focusId: input.id,
        errId: `${input.id}-error`,
      });
    }
  }
  if (!hasValue) {
    const firstInput = container.querySelector("input");
    setGroupError(
      listId,
      errId,
      emptyMessage,
      firstInput ? firstInput.id : addButtonId
    );
  }
}

function validateForm() {
  clearValidationUi();
  const intervalValue = el("poll-interval").value.trim();
  const interval = Number(intervalValue);
  if (
    intervalValue === "" ||
    !Number.isFinite(interval) ||
    !Number.isInteger(interval) ||
    interval < 60
  ) {
    setFieldError("poll-interval", "Enter a poll interval of at least 60 seconds.");
  }
  const sender = el("sender").value.trim();
  if (sender === "") setFieldError("sender", "Enter a sender email address.");
  else if (!EMAIL_RE.test(sender)) setFieldError("sender", "Enter a valid sender email address.");
  if (el("smtp-host").value.trim() === "") setFieldError("smtp-host", "Enter an SMTP host.");
  const portValue = el("smtp-port").value.trim();
  const port = Number(portValue);
  if (
    portValue === "" ||
    !Number.isFinite(port) ||
    !Number.isInteger(port) ||
    port < 1 ||
    port > 65535
  ) {
    setFieldError("smtp-port", "Enter an SMTP port between 1 and 65535.");
  }
  validateRowList("repos-list", "repos-error", "repo", "Add at least one repository.", "add-repo");
  validateRowList(
    "recipients-list",
    "recipients-error",
    "recipient",
    "Add at least one recipient.",
    "add-recipient"
  );
  return validationMap.size;
}

function showValidationFailures() {
  renderErrorSummary();
  saveUi = { state: "invalid", note: null, count: validationMap.size, savedAt: null };
  el("error-summary").focus();
  renderSaveBar();
}

/* --------------------------------------------------------------- save bar */

function renderSaveBar() {
  const btn = el("save-btn");
  const glyph = el("save-glyph");
  const text = el("save-status-text");
  const review = el("save-review");
  const retry = el("save-retry");
  const bar = el("save-bar");
  bar.dataset.state = dirty && saveUi.state === "pristine" ? "dirty" : saveUi.state;

  btn.classList.remove("primary", "secondary");
  btn.removeAttribute("aria-disabled");
  btn.disabled = !configWritable;
  bar.removeAttribute("aria-busy");
  review.hidden = true;
  retry.hidden = true;
  glyph.replaceChildren();

  let tone = "muted";
  let weight = "secondary";
  let label = "";

  if (saveUi.state === "saving") {
    label = "Saving\u2026";
    const spin = document.createElement("span");
    spin.className = "spinner";
    glyph.appendChild(spin);
    btn.setAttribute("aria-disabled", "true");
    bar.setAttribute("aria-busy", "true");
  } else if (saveUi.state === "error") {
    label = "Couldn't save";
    tone = "danger";
    weight = "primary";
    glyph.appendChild(icon("alert"));
    retry.hidden = !configWritable;
  } else if (saveUi.state === "invalid") {
    label = `${saveUi.count} ${saveUi.count === 1 ? "field needs" : "fields need"} attention`;
    tone = "danger";
    weight = "primary";
    glyph.appendChild(icon("alert"));
    review.hidden = false;
  } else if (saveUi.state === "saved") {
    label = `Saved ${saveUi.savedAt}`;
    tone = "ok";
    glyph.appendChild(icon("check"));
  } else if (dirty) {
    label = "Unsaved changes";
    tone = "warn";
    weight = "primary";
    const dot = document.createElement("span");
    dot.className = "save-dot";
    glyph.appendChild(dot);
  } else if (saveUi.note) {
    label = saveUi.note;
    glyph.appendChild(icon("check"));
  } else {
    label = "All changes saved";
    glyph.appendChild(icon("check"));
  }

  btn.classList.add(weight);
  glyph.className = `save-glyph glyph-${tone}`;
  text.textContent = label;
}

function clearSaveTimers() {
  clearTimeout(saveTimer);
  clearTimeout(noteTimer);
}

async function doSave() {
  if (saving || !configWritable) return;
  saving = true;
  bannerError = null;
  renderBanner();
  saveUi = { state: "saving", note: null, count: 0, savedAt: null };
  renderSaveBar();
  const edit = collectEdit();
  try {
    const res = await fetch("/api/config", {
      method: "PUT",
      headers: {
        "Content-Type": "application/json",
        "X-Requested-With": CSRF_VALUE,
      },
      body: JSON.stringify(edit),
    });
    if (res.status === 401) {
      saving = false;
      pendingSave = true;
      enterLogin("Your session has ended. Sign in again: your changes are kept.");
      return;
    }
    const data = await res.json().catch(() => ({}));
    if (res.ok && data.ok === true) {
      saving = false;
      el("admin-token").value = "";
      el("smtp-password").value = "";
      baseline = semanticModel();
      dirty = false;
      saveUi = { state: "saved", note: null, count: 0, savedAt: utcMinutes(Date.now()) };
      renderSaveBar();
      updateBeforeUnload();
      clearSaveTimers();
      saveTimer = setTimeout(() => {
        if (saveUi.state === "saved") {
          saveUi = { state: "pristine", note: null, count: 0, savedAt: null };
          renderSaveBar();
        }
      }, SAVED_TTL_MS);
      await loadConfig({ focus: false });
      setTimeout(refreshStatus, 1500);
    } else {
      saving = false;
      bannerError =
        (data && data.error) || `Save failed (HTTP ${res.status}).`;
      saveUi = { state: "error", note: null, count: 0, savedAt: null };
      renderSaveBar();
      renderBanner();
    }
  } catch (err) {
    saving = false;
    bannerError = `Could not reach the server (${err.message}).`;
    saveUi = { state: "error", note: null, count: 0, savedAt: null };
    renderSaveBar();
    renderBanner();
  }
}

async function onSaveSubmit(event) {
  event.preventDefault();
  if (saving || !configWritable) return;
  if (!dirty) {
    saveUi = { state: "pristine", note: "No changes to save.", count: 0, savedAt: null };
    renderSaveBar();
    clearTimeout(noteTimer);
    noteTimer = setTimeout(() => {
      if (saveUi.state === "pristine" && saveUi.note) {
        saveUi = { state: "pristine", note: null, count: 0, savedAt: null };
        renderSaveBar();
      }
    }, NOTE_TTL_MS);
    return;
  }
  validateForm();
  if (validationMap.size > 0) {
    showValidationFailures();
    return;
  }
  await doSave();
}

/* ------------------------------------------------------------- auth flows */

function enterLogin(notice) {
  sessionNotice = notice || null;
  authed = false;
  el("logout-btn").hidden = true;
  setView("login", { focus: false });
  el("login-token").focus();
  renderBanner();
}

async function loadConfig(opts = {}) {
  let data = null;
  try {
    const res = await fetch("/api/config");
    if (res.status === 401) {
      enterLogin();
      return false;
    }
    if (!res.ok) {
      bannerError = `Could not load the configuration (HTTP ${res.status}).`;
      renderBanner();
      if (!authed) enterLogin();
      return false;
    }
    data = await res.json();
  } catch (err) {
    bannerError = "Could not reach the server.";
    renderBanner();
    if (!authed) enterLogin();
    return false;
  }
  bannerError = null;
  hydrate(data);
  applyRoute({ focus: opts.focus === true && booted });
  return true;
}

function hydrate(data) {
  authed = true;
  authMode = data.auth_mode || "open";
  configWritable = data.config_writable !== false;
  envManaged = data.env_managed || { smtp_password: false, github_token: false };
  const cfg = data.config || {};
  el("poll-interval").value = cfg.poll_interval_seconds != null ? cfg.poll_interval_seconds : "";
  el("cron-expression").value = cfg.cron_expression || "";
  el("sender").value = cfg.sender || "";
  el("sender-name").value = cfg.sender_name || "";
  el("smtp-host").value = (cfg.smtp && cfg.smtp.host) || "";
  el("smtp-port").value = (cfg.smtp && cfg.smtp.port) != null ? cfg.smtp.port : "";
  el("smtp-encryption").value = (cfg.smtp && cfg.smtp.encryption) || "starttls";
  el("smtp-username").value = (cfg.smtp && cfg.smtp.username) || "";
  el("smtp-password").value = "";
  adminTokenSet = !!(cfg.ui && cfg.ui.admin_token_set);
  el("admin-token").value = "";
  el("admin-disable-auth").checked = false;
  syncAdminState();
  syncSmtpPasswordState();
  renderAdminTokenPill();
  buildRows(el("repos-list"), {
    type: "text",
    ariaLabel: "Repository",
    placeholder: "owner/repo",
    addButtonId: "add-repo",
    values: Array.isArray(cfg.repos) ? cfg.repos : [],
  });
  buildRows(el("recipients-list"), {
    type: "email",
    ariaLabel: "Recipient email address",
    placeholder: "you@example.com",
    addButtonId: "add-recipient",
    values: Array.isArray(cfg.recipients) ? cfg.recipients : [],
  });
  renderReadonly(data);
  if (data.status) {
    lastSnapshot = data.status;
    if (currentView === "status") renderStatusView(data.status);
  }
  el("logout-btn").hidden = authMode !== "token";
  clearValidationUi();
  baseline = semanticModel();
  dirty = false;
  if (saveUi.state !== "saved") {
    clearSaveTimers();
    saveUi = { state: "pristine", note: null, count: 0, savedAt: null };
  }
  renderSaveBar();
  updateBeforeUnload();
  renderBanner();
}

async function doLogin(event) {
  event.preventDefault();
  if (loginBusy) return;
  loginBusy = true;
  const errNode = el("login-error");
  errNode.hidden = true;
  errNode.textContent = "";
  try {
    const res = await fetch("/login", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Requested-With": CSRF_VALUE,
      },
      body: JSON.stringify({ admin_token: el("login-token").value }),
    });
    const data = await res.json().catch(() => ({}));
    if (res.ok) {
      loginBusy = false;
      el("login-token").value = "";
      sessionNotice = null;
      if (pendingSave) {
        pendingSave = false;
        authed = true;
        history.replaceState(null, "", "#settings");
        setView("settings", { focus: true });
        validateForm();
        if (validationMap.size > 0) {
          showValidationFailures();
        } else {
          await doSave();
        }
      } else {
        await loadConfig({ focus: true });
      }
      return;
    }
    if (res.status === 401) {
      errNode.textContent = "That token is not correct.";
      errNode.hidden = false;
      el("login-token").focus();
      el("login-token").select();
    } else if (res.status === 429) {
      errNode.textContent = "Too many attempts. Try again later.";
      errNode.hidden = false;
    } else {
      errNode.textContent =
        (data && data.error) || `Sign in failed (HTTP ${res.status}).`;
      errNode.hidden = false;
    }
  } catch (err) {
    errNode.textContent = "Could not reach the server.";
    errNode.hidden = false;
  }
  loginBusy = false;
}

async function doLogout() {
  try {
    const res = await fetch("/api/logout", {
      method: "POST",
      headers: { "X-Requested-With": CSRF_VALUE },
    });
    if (!res.ok && res.status !== 401) {
      bannerError = `Could not log out (HTTP ${res.status}).`;
      renderBanner();
      return;
    }
  } catch (err) {
    bannerError = "Could not reach the server.";
    renderBanner();
    return;
  }
  pendingSave = false;
  authed = false;
  dirty = false;
  bannerError = null;
  sessionNotice = null;
  lastSnapshot = null;
  clearSaveTimers();
  saveUi = { state: "pristine", note: null, count: 0, savedAt: null };
  updateBeforeUnload();
  resetSettingsForm();
  resetStatusView();
  enterLogin();
}

/* ------------------------------------------------------------------ bind */

function bindShow(btnId, inputId) {
  const btn = el(btnId);
  const input = el(inputId);
  btn.addEventListener("click", () => {
    const show = input.type === "password";
    input.type = show ? "text" : "password";
    btn.textContent = show ? "Hide" : "Show";
    btn.setAttribute("aria-pressed", show ? "true" : "false");
    input.focus();
    input.select();
  });
}

function onFormInput(target) {
  if (target.id && validationMap.has(target.id)) clearEntry(target.id);
  const list = target.closest(".row-list");
  if (list && validationMap.has(list.id)) clearEntry(list.id);
  refreshDirty();
}

function bind() {
  bindShow("login-show", "login-token");
  bindShow("admin-token-show", "admin-token");

  el("login-form").addEventListener("submit", doLogin);

  const form = el("settings-form");
  form.addEventListener("submit", onSaveSubmit);
  form.addEventListener("input", (event) => onFormInput(event.target));
  form.addEventListener("change", (event) => {
    if (event.target.id !== "admin-disable-auth") onFormInput(event.target);
  });

  el("admin-disable-auth").addEventListener("change", () => {
    syncAdminState();
    refreshDirty();
  });

  el("add-repo").addEventListener("click", () => {
    addRow("repos-list", {
      type: "text",
      ariaLabel: "Repository",
      placeholder: "owner/repo",
      addButtonId: "add-repo",
    });
  });

  el("add-recipient").addEventListener("click", () => {
    addRow("recipients-list", {
      type: "email",
      ariaLabel: "Recipient email address",
      placeholder: "you@example.com",
      addButtonId: "add-recipient",
    });
  });

  el("empty-add-repo").addEventListener("click", () => {
    history.replaceState(null, "", "#settings");
    setView("settings", { focus: false });
    const firstInput = el("repos-list").querySelector("input");
    if (firstInput) firstInput.focus();
    else el("add-repo").focus();
  });

  el("save-review").addEventListener("click", () => {
    el("error-summary").focus();
  });

  el("save-retry").addEventListener("click", () => {
    if (dirty) onSaveSubmit(new Event("submit", { cancelable: true }));
  });

  el("logout-btn").addEventListener("click", () => {
    if (dirty) el("logout-dialog").showModal();
    else doLogout();
  });

  el("logout-stay").addEventListener("click", () => {
    el("logout-dialog").close();
  });

  el("logout-confirm").addEventListener("click", () => {
    el("logout-dialog").close();
    dirty = false;
    updateBeforeUnload();
    doLogout();
  });

  el("logout-stay").setAttribute("autofocus", "");
}

bind();
applyRoute({ focus: false });
loadConfig();
booted = true;
setInterval(refreshStatus, REFRESH_MS);