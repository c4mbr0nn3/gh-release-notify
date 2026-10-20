"use strict";

const CSRF_VALUE = "gh-release-notify";

const el = (id) => document.getElementById(id);

const fmtWhen = (iso) => {
  if (!iso) return "unknown";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" }) + " UTC";
};

const clearRows = (container) => {
  while (container.firstChild) container.removeChild(container.firstChild);
};

const makeRow = (container, opts) => {
  const group = document.createElement("div");
  group.className = "ui-row-group";
  group.setAttribute("role", "group");
  const input = document.createElement("input");
  input.type = opts.type;
  input.placeholder = opts.placeholder || "";
  input.setAttribute("aria-label", opts.placeholder);
  input.value = opts.value || "";
  if (opts.type === "email") input.spellcheck = false;
  group.appendChild(input);
  const remove = document.createElement("button");
  remove.type = "button";
  remove.textContent = "Remove";
  remove.setAttribute("aria-label", `Remove ${opts.value || "this entry"}`);
  group.appendChild(remove);
  container.appendChild(group);
  remove.addEventListener("click", () => {
    if (container.children.length > 1) {
      group.remove();
    } else {
      input.value = "";
    }
    markDirty(true);
  });
  return input;
};

const readList = (container) =>
  Array.from(container.querySelectorAll("input"))
    .map((i) => i.value.trim())
    .filter((v) => v.length > 0);

const setMsg = (node, text, kind = "info") => {
  node.textContent = text;
  node.dataset.kind = kind;
};

function markDirty(dirty) {
  el("unsaved").hidden = !dirty;
  document.body.dataset.dirty = dirty ? "true" : "false";
}

const REFRESH_MS = 15000;
let refreshTimer = null;
let savedTimer = null;
let settingsVisible = false;
let pendingSave = false;

function scheduleRefresh() {
  if (refreshTimer !== null) return;
  refreshTimer = setInterval(() => {
    if (!document.hidden) refreshStatus();
  }, REFRESH_MS);
}

let lastAuthMode = null;

function showSettings(show) {
  settingsVisible = show;
  el("status").hidden = !show;
  el("section-nav").hidden = !show;
  el("settings").hidden = !show;
  el("logout-btn").hidden = !show;
  el("readonly").hidden = !show;
}

function applyAuthMode(mode) {
  lastAuthMode = mode;
  if (mode !== "token") el("login-section").hidden = true;
}

function renderStatus(st) {
  const body = el("status-repos-body");
  clearRows(body);
  for (const r of st.repos || []) {
    const tr = document.createElement("tr");
    const td1 = document.createElement("td");
    const slug = document.createElement("code");
    slug.textContent = r.repo;
    td1.appendChild(slug);
    const td2 = document.createElement("td");
    const tag = document.createElement("code");
    tag.textContent = r.last_seen || "\u2014";
    td2.appendChild(tag);
    const td3 = document.createElement("td");
    td3.textContent = r.last_seen ? "seen" : "not yet";
    td3.dataset.state = r.last_seen ? "seen" : "none";
    tr.appendChild(td1);
    tr.appendChild(td2);
    tr.appendChild(td3);
    body.appendChild(tr);
  }
  if ((st.repos || []).length === 0) {
    const tr = document.createElement("tr");
    const td = document.createElement("td");
    td.colSpan = 3;
    td.textContent = "No repositories tracked yet.";
    tr.appendChild(td);
    body.appendChild(tr);
  }
  const lastPoll = el("status-last-poll");
  lastPoll.textContent = st.last_poll_finished_at
    ? fmtWhen(st.last_poll_finished_at)
    : "never";
  lastPoll.dateTime = st.last_poll_finished_at || "";
  const nextPoll = el("status-next-poll");
  nextPoll.textContent = fmtWhen(st.next_poll_at);
  nextPoll.dateTime = st.next_poll_at || "";
}

function render(data) {
  const cfg = data.config;
  const ro = data.readonly;
  const em = data.env_managed || {};

  showSettings(true);
  el("login-section").hidden = true;
  applyAuthMode(data.auth_mode);
  el("logout-btn").hidden = data.auth_mode !== "token";

  setMsg(el("login-error"), "");

  el("poll-interval").value = cfg.poll_interval_seconds;
  el("cron-expression").value = cfg.cron_expression || "";
  el("sender").value = cfg.sender;
  el("sender-name").value = cfg.sender_name || "";
  el("smtp-host").value = cfg.smtp.host;
  el("smtp-port").value = cfg.smtp.port;
  el("smtp-encryption").value = cfg.smtp.encryption;
  el("smtp-username").value = cfg.smtp.username;
  el("smtp-password").value = "";

  const adminSet = cfg.ui.admin_token_set;
  const adminNote = el("admin-token-note");
  if (adminSet) {
    adminNote.textContent = "An admin token is set. Leave empty to keep it, enter a new value to change it, or switch on disable to remove authentication.";
  } else {
    adminNote.textContent = "No admin token is set: anyone who can reach this port can edit the config. Enter a value to set one.";
  }
  el("admin-token").value = "";
  el("admin-token-clear").checked = false;
  el("admin-token").disabled = false;

  const reposList = el("repos-list");
  clearRows(reposList);
  for (const r of cfg.repos) {
    makeRow(reposList, { type: "text", placeholder: "owner/repo", value: r });
  }
  if (reposList.children.length === 0) {
    makeRow(reposList, { type: "text", placeholder: "owner/repo" });
  }

  const recipList = el("recipients-list");
  clearRows(recipList);
  for (const r of cfg.recipients) {
    makeRow(recipList, { type: "email", placeholder: "you@example.com", value: r });
  }
  if (recipList.children.length === 0) {
    makeRow(recipList, { type: "email", placeholder: "you@example.com" });
  }

  renderStatus(data.status);

  el("readonly-state-path").textContent = ro.state_path;
  el("readonly-bind-addr").textContent = cfg.ui.bind_addr;
  el("readonly-ui-port").textContent = cfg.ui.port;

  const writable = data.config_writable !== false;
  el("save-btn").disabled = !writable;
  el("readonly-banner").hidden = writable;

  el("smtp-password").disabled = em.smtp_password === true;
  el("smtp-password-note").textContent = em.smtp_password === true
    ? "Managed by the SMTP_PASSWORD environment variable."
    : "Leave empty to keep the existing password.";

  el("github-token-note").hidden = em.github_token !== true;

  markDirty(false);
}

function collectEdit() {
  const edit = {
    poll_interval_seconds: Number(el("poll-interval").value),
    cron_expression: el("cron-expression").value.trim() || null,
    sender: el("sender").value.trim(),
    sender_name: el("sender-name").value.trim() || null,
    repos: readList(el("repos-list")),
    recipients: readList(el("recipients-list")),
    smtp_host: el("smtp-host").value.trim(),
    smtp_port: Number(el("smtp-port").value),
    smtp_encryption: el("smtp-encryption").value,
    smtp_username: el("smtp-username").value.trim(),
  };
  const clearToken = el("admin-token-clear").checked;
  if (clearToken) {
    edit.admin_token = "";
  } else {
    const tok = el("admin-token").value;
    if (tok) edit.admin_token = tok;
  }
  const pw = el("smtp-password").value;
  if (pw) edit.smtp_password = pw;
  return edit;
}

async function refreshStatus() {
  if (!settingsVisible) return;
  try {
    const res = await fetch("/api/config");
    if (!res.ok) return;
    const data = await res.json();
    if (data.status) renderStatus(data.status);
  } catch (e) {
    void e;
  }
}

async function loadConfig() {
  try {
    const res = await fetch("/api/config");
    if (res.status === 401) {
      showSettings(false);
      if (lastAuthMode === null || lastAuthMode === "token") {
        el("login-section").hidden = false;
      }
      setMsg(el("login-error"), "");
      return;
    }
    const data = await res.json();
    render(data);
  } catch (e) {
    showSettings(false);
    el("login-section").hidden = false;
    setMsg(el("login-error"), "Cannot reach the server. " + e.message, "error");
  }
}

async function saveConfig(ev) {
  ev.preventDefault();
  const msg = el("settings-message");
  setMsg(msg, "Saving\u2026", "info");
  let edit;
  try {
    edit = collectEdit();
  } catch (e) {
    setMsg(msg, "Error: " + e.message, "error");
    return;
  }
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
      pendingSave = true;
      setMsg(msg, "Session expired. Sign in again: your edits are kept.", "error");
      el("login-section").hidden = false;
      return;
    }
    const data = await res.json().catch(() => ({}));
    if (res.ok && data.ok) {
      setMsg(msg, "Saved.", "ok");
      markDirty(false);
      await loadConfig();
      clearTimeout(savedTimer);
      savedTimer = setTimeout(() => {
        if (msg.dataset.kind === "ok") setMsg(msg, "", "info");
      }, 4000);
      setTimeout(refreshStatus, 1500);
    } else {
      setMsg(msg, data.error || ("Save failed (HTTP " + res.status + ")"), "error");
    }
  } catch (e) {
    setMsg(msg, "Save failed: " + e.message, "error");
  }
}

async function doLogin(ev) {
  ev.preventDefault();
  const err = el("login-error");
  setMsg(err, "Signing in\u2026", "info");
  const body = { admin_token: el("login-token").value };
  try {
    const res = await fetch("/login", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Requested-With": CSRF_VALUE,
      },
      body: JSON.stringify(body),
    });
    const data = await res.json().catch(() => ({}));
    if (res.ok) {
      el("login-token").value = "";
      setMsg(err, "", "info");
      if (pendingSave) {
        pendingSave = false;
        el("login-section").hidden = true;
        await saveConfig(new Event("submit"));
      } else {
        await loadConfig();
      }
    } else {
      setMsg(err, data.error || ("Sign in failed (HTTP " + res.status + ")"), "error");
    }
  } catch (e) {
    setMsg(err, "Sign in failed: " + e.message, "error");
  }
}

async function doLogout() {
  try {
    const res = await fetch("/api/logout", {
      method: "POST",
      headers: { "X-Requested-With": CSRF_VALUE },
    });
    if (!res.ok) {
      const data = await res.json().catch(() => ({}));
      setMsg(el("settings-message"), data.error || ("Logout failed (HTTP " + res.status + ")"), "error");
      return;
    }
  } catch (e) {
    setMsg(el("settings-message"), "Logout failed: " + e.message, "error");
    return;
  }
  pendingSave = false;
  markDirty(false);
  setMsg(el("settings-message"), "", "info");
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
  el("admin-token-clear").checked = false;
  el("admin-token").disabled = false;
  clearRows(el("repos-list"));
  clearRows(el("recipients-list"));
  clearRows(el("status-repos-body"));
  el("status-last-poll").textContent = "unknown";
  el("status-next-poll").textContent = "unknown";
  showSettings(false);
  el("login-section").hidden = false;
  setMsg(el("login-error"), "");
}

function addRow(listId, opts) {
  const list = el(listId);
  makeRow(list, opts);
  list.lastElementChild.querySelector("input").focus();
  markDirty(true);
}

function bind() {
  el("login-form").addEventListener("submit", doLogin);
  el("settings-form").addEventListener("submit", saveConfig);
  el("settings-form").addEventListener("input", () => markDirty(true));
  el("settings-form").addEventListener("change", () => markDirty(true));
  el("logout-btn").addEventListener("click", doLogout);
  el("add-repo").addEventListener("click", () => addRow("repos-list", { type: "text", placeholder: "owner/repo" }));
  el("add-recipient").addEventListener("click", () => addRow("recipients-list", { type: "email", placeholder: "you@example.com" }));
  const clearBox = el("admin-token-clear");
  const tok = el("admin-token");
  clearBox.addEventListener("change", () => {
    tok.disabled = clearBox.checked;
  });
  tok.disabled = clearBox.checked;
}

bind();
loadConfig();
scheduleRefresh();