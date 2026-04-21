// Get API base path from window location or default to /api
const API_BASE = (() => {
  // Check if we're in a subpath (e.g., /dashboard/api)
  const path = window.location.pathname;
  if (path !== "/" && path.endsWith("/")) {
    return path.slice(0, -1) + "/api";
  } else if (path !== "/") {
    return path + "/api";
  }
  return "/api";
})();

/** @returns {{ queueName: string, jobId: string } | null} */
function parseJobRoute(hash) {
  const h = hash || "";
  const prefix = "#/queue/";
  if (!h.startsWith(prefix)) return null;
  const rest = h.slice(prefix.length);
  const marker = "/job/";
  const idx = rest.indexOf(marker);
  if (idx === -1) return null;
  const queueEnc = rest.slice(0, idx);
  const jobId = rest.slice(idx + marker.length).split(/[?#]/)[0];
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(jobId)
  ) {
    return null;
  }
  try {
    return { queueName: decodeURIComponent(queueEnc), jobId };
  } catch {
    return null;
  }
}

function jobRouteHash(queueName, jobId) {
  return `#/queue/${encodeURIComponent(queueName)}/job/${jobId}`;
}

function clearHashRoute() {
  const { pathname, search } = window.location;
  if (!parseJobRoute(window.location.hash)) return;
  window.history.pushState(null, "", pathname + search);
}

let jobDetailActive = false;
/** @type {object | null} */
let detailJob = null;

const JOB_DETAIL_AUTO_KEY = "chainmq-job-detail-auto-refresh";
let jobDetailLastFetchedAt = null;
let jobDetailRelativeTimer = 0;
/** @type {"details" | "logs"} */
let jobDetailSelectedTab = "details";

function getJobDetailAutoRefresh() {
  const v = localStorage.getItem(JOB_DETAIL_AUTO_KEY);
  if (v === null) return true;
  return v !== "0";
}

function setJobDetailAutoRefresh(on) {
  localStorage.setItem(JOB_DETAIL_AUTO_KEY, on ? "1" : "0");
}

function stopJobDetailRelativeTimer() {
  if (jobDetailRelativeTimer) {
    window.clearInterval(jobDetailRelativeTimer);
    jobDetailRelativeTimer = 0;
  }
}

function startJobDetailRelativeTimer() {
  stopJobDetailRelativeTimer();
  jobDetailRelativeTimer = window.setInterval(updateJobDetailUpdatedLabel, 1000);
}

function touchJobDetailFetched() {
  jobDetailLastFetchedAt = Date.now();
  updateJobDetailUpdatedLabel();
  if (jobDetailActive) {
    startJobDetailRelativeTimer();
  }
}

function updateJobDetailUpdatedLabel() {
  const el = document.getElementById("jobDetailUpdatedLabel");
  if (!el) return;
  if (!jobDetailLastFetchedAt) {
    el.textContent = "Updated —";
    return;
  }
  const sec = Math.max(0, Math.floor((Date.now() - jobDetailLastFetchedAt) / 1000));
  el.textContent =
    sec < 1 ? "Updated just now" : `Updated ${sec}s ago`;
}

function updateJobDetailToolbarMeta(job) {
  const wrap = document.getElementById("jobDetailToolbarMeta");
  const nameEl = document.getElementById("jobDetailToolbarName");
  const idEl = document.getElementById("jobDetailToolbarId");
  if (!wrap || !nameEl || !idEl) return;
  wrap.hidden = false;
  nameEl.textContent = job.name ?? "";
  const jid = String(job.id ?? "");
  idEl.textContent = jid ? truncateMiddle(jid, 8, 6) : "";
  idEl.title = jid;
}

function hideJobDetailToolbarMeta() {
  const wrap = document.getElementById("jobDetailToolbarMeta");
  if (wrap) wrap.hidden = true;
}

/** @param {string | null | undefined} iso */
function formatClockFromIso(iso) {
  if (iso == null) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  return d.toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/** Readable date+time for lifecycle nodes (e.g. "Apr 21, 2:01:30 PM"). */
/** @param {string | null | undefined} iso */
function formatLifecycleWhen(iso) {
  if (iso == null) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    second: "2-digit",
  });
}

/** @param {string | null | undefined} iso */
function parseIsoMs(iso) {
  if (iso == null) return NaN;
  const n = new Date(iso).getTime();
  return Number.isFinite(n) ? n : NaN;
}

function truncateText(s, max) {
  const str = String(s);
  if (str.length <= max) return str;
  return str.slice(0, max - 1) + "…";
}

/** @param {unknown} b */
function formatBackoffHuman(b) {
  if (!b || typeof b !== "object") return "—";
  const o = /** @type {Record<string, Record<string, number>>} */ (b);
  if (o.Fixed) return `Fixed (${o.Fixed.seconds}s)`;
  if (o.Exponential)
    return `Exponential (base ${o.Exponential.base}, cap ${o.Exponential.cap}s)`;
  if (o.Linear)
    return `Linear (+${o.Linear.increment}s, cap ${o.Linear.cap}s)`;
  return truncateText(JSON.stringify(b), 120);
}

/** @param {object} job */
function renderOptionsKvHtml(job) {
  const o = job.options ?? {};
  const delay =
    o.delay_secs != null && o.delay_secs !== "" ? `${o.delay_secs}s` : "None";
  const pri = o.priority != null ? String(o.priority) : "—";
  const retries = o.attempts != null ? String(o.attempts) : "—";
  const backoff = formatBackoffHuman(o.backoff);
  const timeout =
    o.timeout_secs != null ? `${o.timeout_secs}s` : null;
  /** @type {[string, string][]} */
  const rows = [
    ["Delay", delay],
    ["Priority", pri],
    ["Retries", retries],
    ["Backoff", backoff],
  ];
  if (timeout != null) rows.push(["Timeout", timeout]);
  if (o.rate_limit_key != null && o.rate_limit_key !== "")
    rows.push(["Rate limit key", String(o.rate_limit_key)]);
  const fifoHint =
    "Priority is reserved; the wait queue is FIFO for now.";
  return rows
    .map(([k, v]) => {
      const hintTitle = k === "Priority" ? ` title="${escapeAttr(fifoHint)}"` : "";
      const hintClass = k === "Priority" ? " job-detail-kv-row--priority-hint" : "";
      return `<div class="job-detail-kv-row${hintClass}"${hintTitle}><span class="job-detail-kv-k">${escapeHtml(k)}</span><span class="job-detail-kv-v">${escapeHtml(String(v))}</span></div>`;
    })
    .join("");
}

/** @param {unknown} payload */
function renderPayloadStructuredHtml(payload) {
  if (payload == null)
    return '<p class="job-detail-muted">No payload</p>';
  if (typeof payload !== "object" || Array.isArray(payload)) {
    const kind = Array.isArray(payload)
      ? `Array · ${payload.length} items`
      : typeof payload;
    return `<p class="job-detail-payload-summary">${escapeHtml(kind)}</p><p class="job-detail-hint">Use <strong>View JSON</strong> for the full value.</p>`;
  }
  const keys = Object.keys(payload);
  if (keys.length === 0)
    return '<p class="job-detail-muted">Empty object</p>';
  const max = 32;
  const slice = keys.slice(0, max);
  const body = slice
    .map((k) => {
      const v = /** @type {Record<string, unknown>} */ (payload)[k];
      let display;
      if (v === null) display = "null";
      else if (Array.isArray(v)) display = `Array(${v.length})`;
      else if (typeof v === "object") display = "{…}";
      else if (typeof v === "string") display = truncateText(v, 200);
      else display = String(v);
      return `<div class="job-detail-kv-row"><span class="job-detail-kv-k">${escapeHtml(k)}</span><span class="job-detail-kv-v">${escapeHtml(display)}</span></div>`;
    })
    .join("");
  const more =
    keys.length > max
      ? `<p class="job-detail-hint">+ ${keys.length - max} more keys — open JSON view.</p>`
      : "";
  return `<div class="job-detail-kv">${body}</div>${more}`;
}

/** @param {object} job */
function buildRunSummaryLine(job) {
  const state = String(job.state);
  const attempts = job.attempts;
  const maxA = job.options && job.options.attempts != null ? job.options.attempts : "—";
  const created = parseIsoMs(job.created_at);
  const end =
    parseIsoMs(job.failed_at) ||
    parseIsoMs(job.completed_at) ||
    NaN;
  const wall =
    Number.isFinite(created) && Number.isFinite(end)
      ? formatDurationMs(end - created)
      : "—";

  let outcome = "";
  if (state === "Failed") {
    outcome = job.last_error
      ? truncateText(String(job.last_error), 96)
      : "Failed";
  } else if (state === "Completed") {
    outcome =
      job.response != null
        ? "Returned JSON response"
        : "Completed without output";
  } else if (state === "Active") {
    outcome = "In progress";
  } else if (state === "Waiting" || state === "Delayed") {
    outcome = "Not started yet";
  } else if (state === "Paused") {
    outcome = "Paused";
  } else {
    outcome = state;
  }

  const attemptBit = `${attempts} / ${maxA} attempts`;
  return `${outcome} · Wall ${wall} · ${attemptBit}`;
}

function jobDetailCloseOverflowMenu() {
  const d = document.getElementById("jobDetailOverflow");
  if (d) d.open = false;
}

async function fetchJobLogs(jobId) {
  const response = await fetch(
    `${API_BASE}/jobs/${encodeURIComponent(jobId)}/logs?limit=200`,
  );
  if (response.status === 404) {
    return { ok: true, lines: [] };
  }
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body.error || `HTTP ${response.status}`);
  }
  const data = await response.json().catch(() => ({}));
  const lines = Array.isArray(data.lines) ? data.lines : [];
  return { ok: true, lines };
}

function switchJobDetailTab(which) {
  jobDetailSelectedTab = which === "logs" ? "logs" : "details";
  const detailsBtn = document.getElementById("jobDetailTabDetails");
  const logsBtn = document.getElementById("jobDetailTabLogs");
  const detailsPanel = document.getElementById("jobDetailPanelDetails");
  const logsPanel = document.getElementById("jobDetailPanelLogs");
  if (!detailsBtn || !logsBtn || !detailsPanel || !logsPanel) return;
  const showLogs = jobDetailSelectedTab === "logs";
  detailsBtn.setAttribute("aria-selected", (!showLogs).toString());
  logsBtn.setAttribute("aria-selected", showLogs.toString());
  detailsPanel.hidden = showLogs;
  logsPanel.hidden = !showLogs;
  if (showLogs) logsBtn.focus();
}

async function loadJobLogsPanel(jobId) {
  const body = document.getElementById("jobDetailLogsBody");
  if (!body) return;
  body.innerHTML =
    '<p class="job-detail-logs-loading job-detail-muted">Loading logs…</p>';
  try {
    const { lines } = await fetchJobLogs(jobId);
    if (!lines.length) {
      body.innerHTML = `
        <div class="job-detail-logs-empty" role="status">
          <div class="job-detail-logs-empty-icon" aria-hidden="true">
            <svg width="36" height="36" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/><path d="M8 13h6"/><path d="M8 17h4"/></svg>
          </div>
          <p class="job-detail-logs-empty-lead">No log lines for this job yet</p>
          <p class="job-detail-logs-empty-copy">Connect your worker or logging pipeline to <code class="job-detail-inline-code">GET …/jobs/{id}/logs</code>, or check server stdout while jobs run.</p>
        </div>`;
      return;
    }
    body.innerHTML = lines
      .map((line) => {
        const ts = escapeHtml(line.ts ?? "—");
        const level = escapeHtml(String(line.level ?? "info"));
        const msg = escapeHtml(String(line.message ?? ""));
        return `<div class="job-detail-log-line job-detail-log-line--${level.toLowerCase()}"><span class="job-detail-log-ts">${ts}</span><span class="job-detail-log-level">${level}</span><span class="job-detail-log-msg">${msg}</span></div>`;
      })
      .join("");
  } catch (e) {
    body.innerHTML = `<p class="job-detail-error">${escapeHtml("Could not load logs: " + e.message)}</p>`;
  }
}

function escapeAttr(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function truncateMiddle(s, startLen, endLen) {
  const str = String(s);
  if (!str || str.length <= startLen + endLen + 1) return str;
  return `${str.slice(0, startLen)}…${str.slice(-endLen)}`;
}

/** @param {number} ms */
function formatDurationMs(ms) {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  const sec = Math.round(ms / 1000);
  if (sec < 60) return `${sec}s`;
  const min = Math.floor(sec / 60);
  const rs = sec % 60;
  if (min < 60) return rs ? `${min}m ${rs}s` : `${min}m`;
  const h = Math.floor(min / 60);
  const rm = min % 60;
  return rm ? `${h}h ${rm}m` : `${h}h`;
}

/** @param {number} executeAtMs */
function formatDelayCountdown(executeAtMs) {
  const ms = executeAtMs - Date.now();
  if (ms <= 0) {
    return { text: "Due now — waiting to process", variant: "overdue" };
  }
  const totalSec = Math.max(1, Math.ceil(ms / 1000));
  if (totalSec < 60) {
    return { text: `Runs in ${totalSec}s`, variant: "soon" };
  }
  const m = Math.floor(totalSec / 60);
  const s = totalSec % 60;
  if (totalSec < 3600) {
    return { text: `Runs in ${m}m ${s}s`, variant: "normal" };
  }
  const h = Math.floor(totalSec / 3600);
  const rem = totalSec % 3600;
  const m2 = Math.floor(rem / 60);
  const s2 = rem % 60;
  return { text: `Runs in ${h}h ${m2}m ${s2}s`, variant: "normal" };
}

function updateDelayedCountdowns() {
  document.querySelectorAll("[data-execute-at-ms]").forEach((el) => {
    const raw = el.getAttribute("data-execute-at-ms");
    const t = raw ? parseInt(raw, 10) : NaN;
    if (Number.isNaN(t)) return;
    const { text, variant } = formatDelayCountdown(t);
    el.textContent = text;
    if (variant === "overdue") {
      el.style.color = "var(--warning-color)";
      el.style.fontWeight = "600";
    } else if (variant === "soon") {
      el.style.color = "var(--warning-color)";
      el.style.fontWeight = "600";
    } else {
      el.style.color = "var(--primary-color)";
      el.style.fontWeight = "600";
    }
  });
}

let queues = [];
let currentQueue = "";
let currentState = "waiting";
let currentJobId = null;
let currentPage = 1;
let pageSize = 25;
let searchQuery = "";
let allJobs = [];
/** @type {Set<string>} */
const selectedJobIds = new Set();

function clearJobSelection() {
  selectedJobIds.clear();
  const allCb = document.getElementById("selectAllJobsCheckbox");
  if (allCb) {
    allCb.checked = false;
    allCb.indeterminate = false;
  }
  updateBulkActionsBar();
}

function pruneStaleJobSelections() {
  const valid = new Set(allJobs.map((j) => j.id));
  for (const id of selectedJobIds) {
    if (!valid.has(id)) {
      selectedJobIds.delete(id);
    }
  }
}

function getSelectedJobRecords() {
  return allJobs.filter((j) => selectedJobIds.has(j.id));
}

function syncSelectAllCheckbox() {
  const cb = document.getElementById("selectAllJobsCheckbox");
  if (!cb) return;
  const filtered = getFilteredJobs();
  if (filtered.length === 0) {
    cb.checked = false;
    cb.indeterminate = false;
    cb.disabled = true;
    return;
  }
  cb.disabled = false;
  const nSel = filtered.filter((j) => selectedJobIds.has(j.id)).length;
  cb.checked = nSel === filtered.length;
  cb.indeterminate = nSel > 0 && nSel < filtered.length;
}

function updateBulkActionsBar() {
  const bar = document.getElementById("jobsBulkBar");
  const countEl = document.getElementById("bulkSelectionCount");
  const retryBtn = document.getElementById("bulkRetryBtn");
  if (!bar || !countEl || !retryBtn) return;

  const records = getSelectedJobRecords();
  const n = records.length;
  if (n === 0) {
    bar.style.display = "none";
    return;
  }
  bar.style.display = "flex";
  countEl.textContent =
    n === 1 ? "1 job selected" : `${n} jobs selected`;
  const failedCount = records.filter((j) => j.state === "Failed").length;
  retryBtn.style.display = failedCount > 0 ? "inline-flex" : "none";
}

// Theme management
function initTheme() {
  const savedTheme = localStorage.getItem("theme") || "dark";
  document.documentElement.setAttribute("data-theme", savedTheme);
  updateThemeIcon(savedTheme);
}

function toggleTheme() {
  const currentTheme = document.documentElement.getAttribute("data-theme");
  const newTheme = currentTheme === "light" ? "dark" : "light";
  document.documentElement.setAttribute("data-theme", newTheme);
  localStorage.setItem("theme", newTheme);
  updateThemeIcon(newTheme);
}

function updateThemeIcon(theme) {
  const icon = document.getElementById("themeIcon");
  if (!icon) return;

  if (theme === "dark") {
    icon.innerHTML = `
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>
    `;
  } else {
    icon.innerHTML = `
      <circle cx="12" cy="12" r="5"/>
      <path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/>
    `;
  }
}

// Initialize
document.addEventListener("DOMContentLoaded", () => {
  initTheme();
  loadQueues();
  setupEventListeners();

  const autoCb = document.getElementById("jobDetailAutoRefresh");
  if (autoCb) {
    autoCb.checked = getJobDetailAutoRefresh();
    autoCb.addEventListener("change", (e) => {
      setJobDetailAutoRefresh(/** @type {HTMLInputElement} */ (e.target).checked);
    });
  }

  // Auto-refresh every 3 seconds
  setInterval(() => {
    if (jobDetailActive && currentJobId) {
      if (getJobDetailAutoRefresh()) {
        void loadJobDetailPageContent({ silent: true });
      }
    } else if (currentQueue) {
      loadQueueStats();
      loadJobs();
    } else {
      loadQueues();
    }
  }, 3000);

  // Live countdown for delayed jobs (independent of job list refresh)
  setInterval(updateDelayedCountdowns, 1000);

  window.addEventListener("hashchange", () => {
    const parsed = parseJobRoute(window.location.hash);
    if (parsed) {
      if (
        jobDetailActive &&
        currentJobId === parsed.jobId &&
        currentQueue === parsed.queueName
      ) {
        return;
      }
      void openJobDetailPage(parsed.queueName, parsed.jobId, {
        fromHash: true,
      });
    } else if (jobDetailActive) {
      closeJobDetailPage({ skipHashClear: true });
    }
  });
});

function setupEventListeners() {
  // Theme toggle
  document.getElementById("themeToggle").addEventListener("click", toggleTheme);

  // Refresh button
  document.getElementById("refreshBtn").addEventListener("click", () => {
    if (jobDetailActive && currentJobId) {
      void loadJobDetailPageContent({ silent: true });
    } else if (currentQueue) {
      loadQueueStats();
      loadJobs();
    } else {
      loadQueues();
    }
  });

  // Stat cards / state rail — keyboard accessible tabs
  document.querySelectorAll(".stat-card").forEach((card) => {
    card.tabIndex = 0;
    card.addEventListener("click", () => {
      const state = card.dataset.state;
      if (state && currentQueue) {
        switchState(state);
      }
    });
    card.addEventListener("keydown", (e) => {
      if (e.key !== "Enter" && e.key !== " ") return;
      e.preventDefault();
      const state = card.dataset.state;
      if (state && currentQueue) {
        switchState(state);
      }
    });
  });

  // Search input
  const searchInput = document.getElementById("searchInput");
  let searchTimeout;
  searchInput.addEventListener("input", (e) => {
    clearTimeout(searchTimeout);
    searchTimeout = setTimeout(() => {
      searchQuery = e.target.value.toLowerCase();
      currentPage = 1;
      renderJobs();
    }, 300);
  });

  // Page size select
  document.getElementById("pageSizeSelect").addEventListener("change", (e) => {
    pageSize = parseInt(e.target.value);
    currentPage = 1;
    renderJobs();
  });

  // Pagination
  document.getElementById("prevPage").addEventListener("click", () => {
    if (currentPage > 1) {
      currentPage--;
      renderJobs();
    }
  });

  document.getElementById("nextPage").addEventListener("click", () => {
    const totalPages = Math.ceil(getFilteredJobs().length / pageSize);
    if (currentPage < totalPages) {
      currentPage++;
      renderJobs();
    }
  });

  // Queue actions
  document.getElementById("cleanBtn").addEventListener("click", cleanQueue);
  document
    .getElementById("processDelayedBtn")
    .addEventListener("click", processDelayed);
  document
    .getElementById("recoverStalledBtn")
    .addEventListener("click", recoverStalled);
  document.getElementById("retryJobBtn").addEventListener("click", retryJob);
  document.getElementById("deleteJobBtn").addEventListener("click", () =>
    deleteJob(),
  );
  document
    .getElementById("jobDetailBackBtn")
    .addEventListener("click", () => closeJobDetailPage());
  document
    .getElementById("jobDetailCopyLinkBtn")
    ?.addEventListener("click", () => {
      jobDetailCloseOverflowMenu();
      copyJobDetailLink();
    });
  document.getElementById("jobDetailCopyIdBtn")?.addEventListener("click", () => {
    jobDetailCloseOverflowMenu();
    const id = detailJob?.id ?? currentJobId;
    if (id == null) return;
    const btn = document.getElementById("jobDetailCopyIdBtn");
    void navigator.clipboard.writeText(String(id)).then(
      () => {
        btn?.classList.add("job-detail-copied");
        window.setTimeout(() => btn?.classList.remove("job-detail-copied"), 1600);
      },
      () => {},
    );
  });
  document.getElementById("jobDetailRefreshBtn")?.addEventListener("click", () => {
    if (jobDetailActive && currentJobId) {
      void loadJobDetailPageContent({ silent: true });
    }
  });

  document.addEventListener("click", (e) => {
    const ov = document.getElementById("jobDetailOverflow");
    if (!ov || !ov.open) return;
    const t = /** @type {Node} */ (e.target);
    if (ov.contains(t)) return;
    ov.open = false;
  });

  const selectAllCb = document.getElementById("selectAllJobsCheckbox");
  if (selectAllCb) {
    selectAllCb.addEventListener("change", (e) => {
      const checked = e.currentTarget.checked;
      const filtered = getFilteredJobs();
      if (checked) {
        filtered.forEach((j) => selectedJobIds.add(j.id));
      } else {
        filtered.forEach((j) => selectedJobIds.delete(j.id));
      }
      renderJobs();
    });
  }

  document.getElementById("bulkClearBtn")?.addEventListener("click", () => {
    clearJobSelection();
    renderJobs();
  });

  document.getElementById("bulkDeleteBtn")?.addEventListener("click", () => {
    void deleteSelectedJobs();
  });

  document.getElementById("bulkRetryBtn")?.addEventListener("click", () => {
    void retrySelectedJobs();
  });

  // Job table: delete/retry/details (delegation — survives row re-renders)
  const jobsTable = document.querySelector(".jobs-table");
  if (!jobsTable) {
    console.error("[chainmq] .jobs-table not found");
  }
  jobsTable?.addEventListener("change", (e) => {
    const t = e.target;
    if (t.classList?.contains("job-select-cb")) {
      const id = t.dataset.jobId;
      if (t.checked) {
        selectedJobIds.add(id);
      } else {
        selectedJobIds.delete(id);
      }
      syncSelectAllCheckbox();
      updateBulkActionsBar();
    }
  });

  document.getElementById("job-detail-view")?.addEventListener("click", (e) => {
    const tabBtn = e.target.closest("[data-job-tab]");
    if (tabBtn) {
      e.preventDefault();
      const tab = tabBtn.getAttribute("data-job-tab");
      if (tab === "logs" || tab === "details") {
        switchJobDetailTab(tab);
        if (tab === "logs" && currentJobId) void loadJobLogsPanel(currentJobId);
      }
      return;
    }
    const modeBtn = e.target.closest("[data-json-mode-toggle]");
    if (modeBtn) {
      e.preventDefault();
      const id = modeBtn.getAttribute("data-json-mode-toggle");
      const host = document.querySelector(`[data-json-mode-section="${id}"]`);
      if (host) {
        const on = host.classList.toggle("job-detail-show-json");
        modeBtn.textContent = on ? "View fields" : "View JSON";
        modeBtn.setAttribute("aria-pressed", on ? "true" : "false");
      }
      return;
    }
    const logsCta = e.target.closest("[data-focus-tab='logs']");
    if (logsCta) {
      e.preventDefault();
      switchJobDetailTab("logs");
      if (currentJobId) void loadJobLogsPanel(currentJobId);
      return;
    }
    const retryFromEmpty = e.target.closest('[data-action="retry-from-empty"]');
    if (retryFromEmpty) {
      e.preventDefault();
      document.getElementById("retryJobBtn")?.click();
      return;
    }
    const copyBtn = e.target.closest("[data-copy-text], .job-detail-copy-json");
    if (!copyBtn) return;
    let text = copyBtn.getAttribute("data-copy-text");
    if (copyBtn.classList.contains("job-detail-copy-json")) {
      const field = copyBtn.getAttribute("data-copy-job-json");
      if (field === "options" && detailJob) {
        text = JSON.stringify(detailJob.options ?? {}, null, 2);
      } else if (field === "payload" && detailJob) {
        text = JSON.stringify(detailJob.payload ?? {}, null, 2);
      } else if (field === "response" && detailJob) {
        text = JSON.stringify(detailJob.response ?? null, null, 2);
      }
    }
    if (text == null || text === "") return;
    e.preventDefault();
    void navigator.clipboard.writeText(text).then(
      () => {
        copyBtn.classList.add("job-detail-copied");
        window.clearTimeout(copyBtn._copyReset);
        copyBtn._copyReset = window.setTimeout(() => {
          copyBtn.classList.remove("job-detail-copied");
        }, 1600);
      },
      () => {},
    );
  });

  jobsTable?.addEventListener("click", (e) => {
    const deleteBtn = e.target.closest('[data-action="delete-job"]');
    if (deleteBtn) {
      e.stopPropagation();
      deleteJobById(deleteBtn.dataset.jobId, deleteBtn.dataset.queueName);
      return;
    }
    const retryBtn = e.target.closest('[data-action="retry-job"]');
    if (retryBtn) {
      e.stopPropagation();
      retryJobById(retryBtn.dataset.jobId);
      return;
    }
    const row = e.target.closest(".job-row");
    if (
      row &&
      !e.target.closest("button, input, label, .col-select, .job-actions")
    ) {
      void openJobDetailPage(currentQueue, row.dataset.jobId);
    }
  });
}

function formatJobsContextLabel(stateKey) {
  const labels = {
    waiting: "Waiting",
    active: "Active",
    delayed: "Delayed",
    failed: "Failed",
    completed: "Completed",
  };
  const label = labels[stateKey] || stateKey;
  return `Showing ${label} jobs`;
}

function switchState(state) {
  clearJobSelection();
  currentState = state;
  currentPage = 1;

  document.querySelectorAll(".stat-card").forEach((card) => {
    const on = card.dataset.state === state;
    card.classList.toggle("active", on);
    card.setAttribute("aria-selected", on ? "true" : "false");
  });

  const ctx = document.getElementById("jobsContextLabel");
  if (ctx) ctx.textContent = formatJobsContextLabel(state);

  loadJobs();
}

async function loadQueues() {
  try {
    const response = await fetch(`${API_BASE}/queues`);
    const data = await response.json();

    queues = data.queues || [];
    renderQueues();

    const route = parseJobRoute(window.location.hash);
    if (route) {
      await openJobDetailPage(route.queueName, route.jobId, { fromHash: true });
    } else if (!currentQueue && queues.length > 0) {
      selectQueue(queues[0]);
    }
  } catch (error) {
    console.error("Failed to load queues:", error);
    document.getElementById("queues-list").innerHTML =
      '<div class="loading-queues">Failed to load queues</div>';
  }
}

function renderQueues() {
  const queuesList = document.getElementById("queues-list");

  if (queues.length === 0) {
    queuesList.innerHTML = '<div class="loading-queues">No queues found</div>';
    return;
  }

  queuesList.innerHTML = queues
    .map((queue) => {
      const isActive = queue === currentQueue;
      return `
      <div class="queue-item ${isActive ? "active" : ""}" data-queue="${queue}">
        <span class="queue-item-name">${queue}</span>
        <span class="queue-item-stats" id="queue-stats-${queue}">-</span>
      </div>
    `;
    })
    .join("");

  queuesList.querySelectorAll(".queue-item").forEach((item) => {
    item.tabIndex = 0;
    item.setAttribute("role", "button");
    item.addEventListener("click", () => {
      selectQueue(item.dataset.queue);
    });
    item.addEventListener("keydown", (e) => {
      if (e.key !== "Enter" && e.key !== " ") return;
      e.preventDefault();
      selectQueue(item.dataset.queue);
    });
  });

  // Load stats for all queues
  queues.forEach((queue) => loadQueueStatsForSidebar(queue));
}

async function loadQueueStatsForSidebar(queueName) {
  try {
    const response = await fetch(`${API_BASE}/queues/${queueName}/stats`);
    const data = await response.json();
    const total =
      (data.waiting || 0) +
      (data.active || 0) +
      (data.delayed || 0) +
      (data.failed || 0);
    const statsEl = document.getElementById(`queue-stats-${queueName}`);
    if (statsEl) {
      statsEl.textContent = total;
    }
  } catch (error) {
    console.error(`Failed to load stats for ${queueName}:`, error);
  }
}

function selectQueue(queueName) {
  jobDetailActive = false;
  currentJobId = null;
  detailJob = null;
  clearHashRoute();

  currentQueue = queueName;
  currentState = "waiting";
  currentPage = 1;
  searchQuery = "";
  document.getElementById("searchInput").value = "";

  // Update UI
  document.getElementById("job-detail-view").style.display = "none";
  document.getElementById("empty-state").style.display = "none";
  document.getElementById("queue-view").style.display = "block";
  document.getElementById("queue-name-display").textContent = queueName;

  const retryBtn = document.getElementById("retryJobBtn");
  if (retryBtn) retryBtn.style.display = "none";

  // Update active queue in sidebar
  document.querySelectorAll(".queue-item").forEach((item) => {
    item.classList.toggle("active", item.dataset.queue === queueName);
  });

  switchState("waiting");

  loadQueueStats();
  loadJobs();
}

async function loadQueueStats() {
  if (!currentQueue) return;

  try {
    const response = await fetch(`${API_BASE}/queues/${currentQueue}/stats`);
    const data = await response.json();

    // Update stat cards
    document.getElementById("stat-waiting").textContent = data.waiting || 0;
    document.getElementById("stat-active").textContent = data.active || 0;
    document.getElementById("stat-delayed").textContent = data.delayed || 0;
    document.getElementById("stat-failed").textContent = data.failed || 0;
    document.getElementById("stat-completed").textContent = data.completed || 0;

    // Update sidebar stats
    loadQueueStatsForSidebar(currentQueue);
  } catch (error) {
    console.error("Failed to load queue stats:", error);
  }
}

async function loadJobs() {
  if (!currentQueue) return;

  try {
    const response = await fetch(
      `${API_BASE}/queues/${currentQueue}/jobs/${currentState}?limit=1000`,
    );

    if (!response.ok) {
      const errorData = await response
        .json()
        .catch(() => ({ error: "Unknown error" }));
      throw new Error(errorData.error || `HTTP ${response.status}`);
    }

    const data = await response.json();

    console.log(
      `Loaded ${
        data.jobs?.length || 0
      } ${currentState} jobs for queue ${currentQueue}`,
    );
    console.log("Jobs data:", data.jobs?.slice(0, 2)); // Debug: show first 2 jobs
    allJobs = data.jobs || [];
    pruneStaleJobSelections();
    renderJobs();
  } catch (error) {
    console.error("Failed to load jobs:", error);
    document.getElementById("jobs-table-body").innerHTML =
      `<tr><td colspan="7" class="loading-jobs">Failed to load jobs: ${error.message}</td></tr>`;
  }
}

function getFilteredJobs() {
  if (!searchQuery) return allJobs;

  return allJobs.filter((job) => {
    const searchStr = `${job.id} ${job.name} ${job.queue_name}`.toLowerCase();
    return searchStr.includes(searchQuery);
  });
}

function renderJobs() {
  const tbody = document.getElementById("jobs-table-body");
  const filteredJobs = getFilteredJobs();
  const totalPages = Math.ceil(filteredJobs.length / pageSize);
  const startIdx = (currentPage - 1) * pageSize;
  const endIdx = startIdx + pageSize;
  const pageJobs = filteredJobs.slice(startIdx, endIdx);

  if (pageJobs.length === 0) {
    tbody.innerHTML = `
      <tr>
        <td colspan="7" class="empty-jobs">
          <div class="empty-jobs-icon">📭</div>
          <div>No ${currentState} jobs found</div>
        </td>
      </tr>
    `;
    document.getElementById("pagination").style.display = "none";
    syncSelectAllCheckbox();
    updateBulkActionsBar();
    return;
  }

  tbody.innerHTML = pageJobs.map((job) => createJobRow(job)).join("");

  // Update pagination
  document.getElementById("pageInfo").textContent = `Page ${currentPage} of ${
    totalPages || 1
  }`;
  document.getElementById("prevPage").disabled = currentPage === 1;
  document.getElementById("nextPage").disabled = currentPage >= totalPages;
  document.getElementById("pagination").style.display =
    totalPages > 1 ? "flex" : "none";

  syncSelectAllCheckbox();
  updateBulkActionsBar();
  updateDelayedCountdowns();
}

function createJobRow(job) {
  const created = new Date(job.created_at).toLocaleString();
  const stateClass = job.state.toLowerCase();

  // For delayed jobs, show when they'll execute
  let timeInfo = created;
  if (job.state === "Delayed" && job.options.delay_secs != null) {
    const executeAtMs =
      new Date(job.created_at).getTime() + Number(job.options.delay_secs) * 1000;
    const executeAt = new Date(executeAtMs);
    const { text: initialCd, variant: initialVariant } =
      formatDelayCountdown(executeAtMs);
    const cdColor =
      initialVariant === "overdue" || initialVariant === "soon"
        ? "var(--warning-color)"
        : "var(--primary-color)";
    timeInfo = `<div>${created}</div><div style="font-size: 12px; color: var(--text-secondary); margin-top: 4px;">Executes: ${executeAt.toLocaleString()}</div><div class="job-delay-countdown" data-execute-at-ms="${executeAtMs}" style="font-size: 13px; font-weight: 600; margin-top: 6px; color: ${cdColor};" aria-live="polite">${initialCd}</div>`;
  } else if (job.state === "Active" && job.started_at) {
    // For active jobs, show how long they've been running
    const started = new Date(job.started_at);
    const now = new Date();
    const elapsed = Math.floor((now - started) / 1000); // seconds
    const timeout = job.options.timeout_secs || 300; // default 5 minutes
    const elapsedMins = Math.floor(elapsed / 60);
    const elapsedSecs = elapsed % 60;
    const timeoutMins = Math.floor(timeout / 60);

    let elapsedStr = `${elapsedMins}m ${elapsedSecs}s`;
    const isStalled = elapsed > timeout;
    if (isStalled) {
      elapsedStr = `<span style="color: var(--danger-color); font-weight: 600;">${elapsedStr} (STALLED)</span>`;
    }

    timeInfo = `<div>${started.toLocaleString()}</div><div style="font-size: 12px; color: var(--text-secondary); margin-top: 4px;">Running: ${elapsedStr} / ${timeoutMins}m timeout</div>`;
  }

  const isSelected = selectedJobIds.has(job.id);
  return `
    <tr class="job-row" data-job-id="${job.id}">
      <td class="col-select">
        <input
          type="checkbox"
          class="job-select-cb"
          data-job-id="${escapeAttr(job.id)}"
          ${isSelected ? "checked" : ""}
          aria-label="Select job ${escapeAttr(job.id.substring(0, 8))}"
        />
      </td>
      <td><span class="job-id">${job.id.substring(0, 8)}...</span></td>
      <td><span class="job-name">${job.name}</span></td>
      <td><span class="job-state-badge ${stateClass}">${job.state}</span></td>
      <td>${timeInfo}</td>
      <td>${job.attempts} / ${job.options.attempts}</td>
      <td>
        <div class="job-actions">
          ${
            job.state === "Failed"
              ? `<button type="button" class="btn btn-success btn-sm" data-action="retry-job" data-job-id="${escapeAttr(job.id)}">Retry</button>`
              : ""
          }
          <button type="button" class="btn btn-danger-outline btn-sm" data-action="delete-job" data-job-id="${escapeAttr(job.id)}" data-queue-name="${escapeAttr(job.queue_name)}">Delete</button>
        </div>
      </td>
    </tr>
  `;
}

async function openJobDetailPage(queueName, jobId, opts = {}) {
  if (!jobId) return;

  if (
    opts.fromHash &&
    jobDetailActive &&
    currentJobId === jobId &&
    currentQueue === queueName
  ) {
    return;
  }

  jobDetailSelectedTab = "details";

  if (!opts.fromHash) {
    window.location.hash = jobRouteHash(queueName, jobId);
  }

  jobDetailActive = true;
  currentJobId = jobId;
  currentQueue = queueName;

  document.getElementById("empty-state").style.display = "none";
  document.getElementById("queue-view").style.display = "none";
  document.getElementById("job-detail-view").style.display = "flex";

  document.querySelectorAll(".queue-item").forEach((item) => {
    item.classList.toggle("active", item.dataset.queue === queueName);
  });

  await loadJobDetailPageContent({ silent: false });
}

async function loadJobDetailPageContent({ silent }) {
  if (!currentJobId) return;
  const container = document.getElementById("jobDetailContent");
  if (!silent && container) {
    container.innerHTML = '<p class="job-detail-loading">Loading job…</p>';
  }
  try {
    const response = await fetch(`${API_BASE}/jobs/${currentJobId}`);
    const body = await response.json().catch(() => ({}));
    if (!response.ok) {
      throw new Error(body.error || `HTTP ${response.status}`);
    }
    const job = body;
    detailJob = job;

    if (job.queue_name && job.queue_name !== currentQueue) {
      currentQueue = job.queue_name;
      document.querySelectorAll(".queue-item").forEach((item) => {
        item.classList.toggle("active", item.dataset.queue === job.queue_name);
      });
    }

    const hashParsed = parseJobRoute(window.location.hash);
    if (
      job.queue_name &&
      hashParsed &&
      hashParsed.jobId === String(job.id) &&
      hashParsed.queueName !== job.queue_name
    ) {
      window.history.replaceState(
        null,
        "",
        window.location.pathname +
          window.location.search +
          jobRouteHash(job.queue_name, String(job.id)),
      );
    }

    renderJobDetailPage(job);
    touchJobDetailFetched();
    updateJobDetailToolbarMeta(job);
    switchJobDetailTab(jobDetailSelectedTab);
    if (jobDetailSelectedTab === "logs") {
      void loadJobLogsPanel(job.id);
    }
    const retryBtn = document.getElementById("retryJobBtn");
    if (retryBtn) {
      retryBtn.style.display = job.state === "Failed" ? "inline-flex" : "none";
    }
  } catch (error) {
    detailJob = null;
    if (container) {
      const p = document.createElement("p");
      p.className = "job-detail-error";
      p.textContent = "Could not load job: " + error.message;
      container.innerHTML = "";
      container.appendChild(p);
    }
    const retryBtn = document.getElementById("retryJobBtn");
    if (retryBtn) retryBtn.style.display = "none";
  }
}

/** @param {object} job */
function renderJobExecutionMetadata(job) {
  const attemptMax =
    job.options && job.options.attempts != null ? job.options.attempts : "—";
  const cur = job.attempts;
  const copySvg = `<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`;
  const workerRow = job.worker_id
    ? `<div class="job-detail-meta-row"><span class="job-detail-meta-label">Worker</span><span class="job-detail-meta-val"><span class="job-detail-mono-clip">${escapeHtml(truncateMiddle(String(job.worker_id), 10, 6))}</span><button type="button" class="job-detail-copy-chip job-detail-copy-chip--ghost" data-copy-text="${escapeAttr(String(job.worker_id))}" aria-label="Copy worker ID" title="Copy full ID">${copySvg}</button></span></div>`
    : `<div class="job-detail-meta-row"><span class="job-detail-meta-label">Worker</span><span class="job-detail-meta-val job-detail-muted">—</span></div>`;
  const err = job.last_error
    ? `<div class="job-detail-meta-row job-detail-meta-row--error"><span class="job-detail-meta-label">Last error</span><div class="job-detail-error-box">${escapeHtml(job.last_error)}</div></div>`
    : "";
  return `<div class="job-detail-exec-metadata">
    <p class="job-detail-exec-metadata-eyebrow">Run metadata</p>
    <div class="job-detail-exec-metadata-inner">
      ${workerRow}
      <div class="job-detail-meta-row job-detail-meta-row--attempts"><span class="job-detail-meta-label">Attempts</span><p class="job-detail-meta-attempts" aria-label="Attempts used"><span class="job-detail-meta-attempts-num">${escapeHtml(String(cur))}</span><span class="job-detail-meta-attempts-sep">/</span><span class="job-detail-meta-attempts-den">${escapeHtml(String(attemptMax))}</span></p></div>
      ${err}
    </div>
  </div>`;
}

/** @param {object} job */
function renderResponseEmptyState(job) {
  const state = String(job.state);
  let lead = "No response was returned for this job";
  /** @type {string} */
  let bodyHtml =
    "<span>Response appears when a worker finishes and sets output.</span>";
  if (state === "Completed") {
    lead = "This job completed without output";
    bodyHtml =
      'Attach JSON with <code class="job-detail-inline-code">JobContext::set_response</code> before the job finishes successfully.';
  } else if (state === "Failed") {
    lead = "No response payload";
    bodyHtml =
      "See <strong>Last error</strong> in execution metadata, or open logs for details.";
  } else if (state === "Active") {
    lead = "No response yet";
    bodyHtml = "The worker has not finished this job.";
  } else if (state === "Waiting" || state === "Delayed") {
    lead = "No response yet";
    bodyHtml = "This job has not completed.";
  }
  const retryBtn =
    state === "Failed"
      ? `<button type="button" class="btn btn-success btn-sm" data-action="retry-from-empty">Retry job</button>`
      : "";
  const tone = String(job.state).toLowerCase();
  return `<div class="job-detail-card job-detail-card--lift job-detail-card--response job-detail-card--response-empty job-detail-card--response-hero job-detail-card--response-tone-${escapeHtml(tone)}">
    <div class="job-detail-block-head">
      <h3 class="job-detail-block-head__title">Response</h3>
    </div>
    <div class="job-detail-response-placeholder-body job-detail-response-placeholder-body--centered">
      <div class="job-detail-response-empty-state" role="note">
        <div class="job-detail-response-empty-icon" aria-hidden="true">
          <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/><path d="M12 11v6"/><path d="M12 18h.01"/></svg>
        </div>
        <p class="job-detail-response-empty-lead">${escapeHtml(lead)}</p>
        <p class="job-detail-response-empty">${bodyHtml}</p>
        <div class="job-detail-empty-ctas">
          <button type="button" class="btn btn-secondary btn-sm" data-focus-tab="logs">View logs</button>
          ${retryBtn}
        </div>
      </div>
    </div>
  </div>`;
}

/** @param {object} job */
function renderLifecycleCard(job) {
  const created = parseIsoMs(job.created_at);
  const started = parseIsoMs(job.started_at);
  const completed = parseIsoMs(job.completed_at);
  const failed = parseIsoMs(job.failed_at);
  const end = Number.isFinite(completed)
    ? completed
    : Number.isFinite(failed)
      ? failed
      : NaN;
  const isFailEnd = Number.isFinite(failed) && !Number.isFinite(completed);

  const wallMs =
    Number.isFinite(created) && Number.isFinite(end) ? end - created : NaN;
  const waitMs =
    Number.isFinite(created) && Number.isFinite(started)
      ? started - created
      : NaN;
  const runMs =
    Number.isFinite(started) && Number.isFinite(end) ? end - started : NaN;

  const iconClock = `<svg class="job-detail-mile-ic" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></svg>`;
  const iconGear = `<svg class="job-detail-mile-ic" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><circle cx="12" cy="12" r="3"/><path d="M12 1v2m0 18v2M4.22 4.22l1.42 1.42m12.72 12.72l1.42 1.42M1 12h2m18 0h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42"/></svg>`;
  const iconCheck = `<svg class="job-detail-mile-ic" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M20 6L9 17l-5-5"/></svg>`;
  const iconFail = `<svg class="job-detail-mile-ic" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M18 6L6 18M6 6l12 12"/></svg>`;

  const milestones = [
    {
      id: "c",
      label: "Created",
      done: Number.isFinite(created),
      timeLabel: formatLifecycleWhen(job.created_at),
      icon: iconClock,
    },
    {
      id: "s",
      label: "Started",
      done: Number.isFinite(started),
      timeLabel: formatLifecycleWhen(job.started_at),
      icon: iconGear,
    },
    {
      id: "f",
      label: isFailEnd ? "Failed" : "Finished",
      done: Number.isFinite(end),
      fail: Boolean(isFailEnd && Number.isFinite(end)),
      timeLabel: formatLifecycleWhen(
        isFailEnd ? job.failed_at : job.completed_at,
      ),
      icon: isFailEnd ? iconFail : iconCheck,
    },
  ];

  let milestoneRow = "";
  for (let i = 0; i < milestones.length; i++) {
    const m = milestones[i];
    const failClass = m.fail && m.done ? " job-detail-te-mile--fail" : "";
    const emoji =
      m.done && m.fail ? "✖" : m.done ? "✔" : "⏱";
    milestoneRow += `<div class="job-detail-te-mile ${m.done ? "job-detail-te-mile--done" : ""}${failClass}" data-mile="${m.id}">
      <span class="job-detail-te-mile-icon" aria-hidden="true">${m.icon}</span>
      <span class="job-detail-te-mile-label"><span class="job-detail-te-mile-emoji" aria-hidden="true">${emoji}</span>${escapeHtml(m.label)}</span>
      <span class="job-detail-te-mile-time">${escapeHtml(m.timeLabel)}</span>
    </div>`;
    if (i < milestones.length - 1) {
      const lit = milestones[i].done && milestones[i + 1].done;
      const dangerConn = lit && milestones[i + 1].fail;
      milestoneRow += `<div class="job-detail-te-mile-conn${lit ? " job-detail-te-mile-conn--lit" : ""}${dangerConn ? " job-detail-te-mile-conn--danger" : ""}" aria-hidden="true"></div>`;
    }
  }

  const durVal = (ms) =>
    Number.isFinite(ms)
      ? `<strong>${escapeHtml(formatDurationMs(ms))}</strong>`
      : `<span class="job-detail-te-dur-na">—</span>`;

  const attemptCur = Number(job.attempts);
  const attemptCurSafe =
    Number.isFinite(attemptCur) && attemptCur >= 0 ? attemptCur : 0;
  const attemptMaxRaw = job.options && job.options.attempts;
  const attemptMax =
    typeof attemptMaxRaw === "number" && attemptMaxRaw > 0
      ? Math.min(attemptMaxRaw, 24)
      : null;

  let attemptsViz = "";
  if (attemptMax != null) {
    const slots = [];
    for (let i = 1; i <= attemptMax; i++) {
      slots.push(
        `<span class="job-detail-te-slot ${i <= attemptCurSafe ? "job-detail-te-slot--used" : ""}" title="Attempt ${i}" aria-hidden="true"></span>`,
      );
    }
    attemptsViz = `<div class="job-detail-te-attempts-viz">
      <span class="job-detail-te-foot-eyebrow">Retry budget</span>
      <div class="job-detail-te-slots">${slots.join("")}</div>
      <span class="job-detail-te-slots-hint">${escapeHtml(String(attemptCurSafe))} of ${escapeHtml(String(attemptMax))} attempts recorded</span>
    </div>`;
  }

  const foot = `<div class="job-detail-te-foot job-detail-te-foot--card">
    <div class="job-detail-te-foot-strip">
      <span class="job-detail-te-foot-eyebrow">Lifecycle</span>
      <div class="job-detail-te-milestone-row job-detail-te-milestone-row--timeline" role="presentation" aria-label="Job lifecycle">${milestoneRow}</div>
    </div>
    <div class="job-detail-te-durations">
      <div class="job-detail-te-dur"><span class="job-detail-te-dur-k">Wall time</span>${durVal(wallMs)}</div>
      <div class="job-detail-te-dur"><span class="job-detail-te-dur-k">Queue wait</span>${durVal(waitMs)}</div>
      <div class="job-detail-te-dur"><span class="job-detail-te-dur-k">Run time</span>${durVal(runMs)}</div>
    </div>
    ${attemptsViz}
  </div>`;

  return `<div class="job-detail-card job-detail-card--lift job-detail-card--lifecycle">
    <div class="job-detail-block-head">
      <h3 class="job-detail-block-head__title">Lifecycle</h3>
    </div>
    <div class="job-detail-lifecycle-body">${foot}</div>
  </div>`;
}

function renderJobDetailPage(job) {
  const container = document.getElementById("jobDetailContent");
  if (!container) return;

  const stateClass = String(job.state).toLowerCase();
  const jid = String(job.id);
  const jidShort = truncateMiddle(jid, 10, 8);
  const pulseClass = stateClass === "completed" ? " job-state-badge--pulse" : "";

  const optionsJson = JSON.stringify(job.options ?? {}, null, 2);
  const payloadJson = JSON.stringify(job.payload ?? {}, null, 2);
  const hasResponse = job.response != null;
  const responseJson = hasResponse
    ? JSON.stringify(job.response, null, 2)
    : "";

  const copyIconSvg = `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>`;

  const responseIsPlainObject =
    hasResponse &&
    job.response !== null &&
    typeof job.response === "object" &&
    !Array.isArray(job.response);
  const responseStructured = responseIsPlainObject
    ? renderPayloadStructuredHtml(job.response)
    : "";

  const responseBlock = hasResponse
    ? responseIsPlainObject
      ? `<div class="job-detail-card job-detail-card--lift job-detail-card--response job-detail-card--response-hero">
    <div class="job-detail-block-head">
      <h3 class="job-detail-block-head__title">Response</h3>
      <div class="job-detail-block-head__actions">
        <button type="button" class="btn btn-ghost btn-sm job-detail-json-mode-btn" data-json-mode-toggle="response" aria-pressed="false">View JSON</button>
        <button type="button" class="job-detail-copy-json job-detail-icon-copy" data-copy-job-json="response" aria-label="Copy Response as JSON" title="Copy JSON">${copyIconSvg}</button>
      </div>
    </div>
    <div class="job-detail-card-body">
      <div data-json-mode-section="response" class="job-detail-duotone">
        <div class="job-detail-struct-wrap">${responseStructured}</div>
        <div class="job-detail-json-wrap"><div class="job-detail-json job-detail-json--tone-response"><pre><code class="job-detail-code job-detail-code--highlighted">${highlightJsonToHtml(responseJson)}</code></pre></div></div>
      </div>
    </div>
  </div>`
      : `<div class="job-detail-card job-detail-card--lift job-detail-card--response job-detail-card--response-hero">
    <div class="job-detail-block-head">
      <h3 class="job-detail-block-head__title">Response</h3>
      <div class="job-detail-block-head__actions">
        <button type="button" class="job-detail-copy-json job-detail-icon-copy" data-copy-job-json="response" aria-label="Copy Response as JSON" title="Copy JSON">${copyIconSvg}</button>
      </div>
    </div>
    <div class="job-detail-response-body-json"><div class="job-detail-json job-detail-json--tone-response job-detail-json--in-card"><pre><code class="job-detail-code job-detail-code--highlighted">${highlightJsonToHtml(responseJson)}</code></pre></div></div>
  </div>`
    : renderResponseEmptyState(job);

  const lifecycleCard = renderLifecycleCard(job);
  const execMetadata = renderJobExecutionMetadata(job);
  const optionsStructured = renderOptionsKvHtml(job);
  const payloadStructured = renderPayloadStructuredHtml(job.payload);

  container.innerHTML = `
  <div class="job-detail-shell">
    <div class="job-detail-tablist" role="tablist" aria-label="Job views">
      <button type="button" class="job-detail-tab" role="tab" id="jobDetailTabDetails" data-job-tab="details" aria-selected="true" aria-controls="jobDetailPanelDetails">Details</button>
      <button type="button" class="job-detail-tab" role="tab" id="jobDetailTabLogs" data-job-tab="logs" aria-selected="false" aria-controls="jobDetailPanelLogs">Logs</button>
    </div>

    <div id="jobDetailPanelDetails" class="job-detail-tabpanel" role="tabpanel" aria-labelledby="jobDetailTabDetails">
      <div class="job-detail-hero job-detail-hero--v2 job-detail-hero--state-${escapeHtml(stateClass)}">
        <div class="job-detail-hero-accent" aria-hidden="true"></div>
        <div class="job-detail-hero-row job-detail-hero-row--primary">
          <h2 class="job-detail-hero-title">${escapeHtml(job.name)}</h2>
          <span class="job-state-badge job-state-badge--hero${pulseClass} ${escapeHtml(stateClass)}">${escapeHtml(String(job.state))}</span>
        </div>
        <p class="job-detail-run-summary job-detail-hero-summary">${escapeHtml(buildRunSummaryLine(job))}</p>
        <div class="job-detail-hero-row job-detail-hero-row--tertiary">
          <span class="job-detail-hero-tertiary-label">Queue</span>
          <span class="job-detail-hero-tertiary-val">${escapeHtml(job.queue_name)}</span>
          <span class="job-detail-hero-tertiary-dot" aria-hidden="true">·</span>
          <code class="job-detail-hero-tertiary-id" title="${escapeAttr(jid)}">${escapeHtml(jidShort)}</code>
          <button type="button" class="job-detail-copy-chip job-detail-copy-chip--mini btn btn-secondary btn-sm" data-copy-text="${escapeAttr(jid)}" aria-label="Copy job ID" title="Copy full job ID">Copy</button>
        </div>
      </div>

      <div class="job-detail-main-grid">
        <div class="job-detail-col job-detail-col--primary">
          ${lifecycleCard}
          ${responseBlock}
        </div>
        <div class="job-detail-col job-detail-col--secondary">
          <div class="job-detail-card job-detail-card--lift job-detail-card--execution">
            <div class="job-detail-block-head">
              <h3 class="job-detail-block-head__title">Execution</h3>
              <div class="job-detail-block-head__actions">
                <button type="button" class="btn btn-ghost btn-sm job-detail-json-mode-btn" data-json-mode-toggle="options" aria-pressed="false">View JSON</button>
                <button type="button" class="job-detail-copy-json job-detail-icon-copy" data-copy-job-json="options" aria-label="Copy Execution options as JSON" title="Copy JSON">${copyIconSvg}</button>
              </div>
            </div>
            <div class="job-detail-card-body-subtle">
              <div data-json-mode-section="options" class="job-detail-duotone">
                <div class="job-detail-struct-wrap job-detail-kv job-detail-kv--exec">${optionsStructured}</div>
                <div class="job-detail-json-wrap"><div class="job-detail-json job-detail-json--tone-options"><pre><code class="job-detail-code job-detail-code--highlighted">${highlightJsonToHtml(optionsJson)}</code></pre></div></div>
              </div>
              <p class="job-detail-hint job-detail-hint--exec-foot">Hover <strong>Priority</strong> for the FIFO note.</p>
            </div>
            ${execMetadata}
          </div>

          <div class="job-detail-card job-detail-card--lift job-detail-card--payload">
            <div class="job-detail-block-head">
              <h3 class="job-detail-block-head__title">Payload</h3>
              <div class="job-detail-block-head__actions">
                <button type="button" class="btn btn-ghost btn-sm job-detail-json-mode-btn" data-json-mode-toggle="payload" aria-pressed="false">View JSON</button>
                <button type="button" class="job-detail-copy-json job-detail-icon-copy" data-copy-job-json="payload" aria-label="Copy Payload as JSON" title="Copy JSON">${copyIconSvg}</button>
              </div>
            </div>
            <div class="job-detail-card-body">
              <div data-json-mode-section="payload" class="job-detail-duotone">
                <div class="job-detail-struct-wrap">${payloadStructured}</div>
                <div class="job-detail-json-wrap"><div class="job-detail-json job-detail-json--tone-payload"><pre><code class="job-detail-code job-detail-code--highlighted">${highlightJsonToHtml(payloadJson)}</code></pre></div></div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <div id="jobDetailPanelLogs" class="job-detail-tabpanel job-detail-tabpanel--logs" role="tabpanel" aria-labelledby="jobDetailTabLogs" hidden>
      <div class="job-detail-logs-panel">
        <p class="job-detail-logs-intro job-detail-muted">Inline log lines from your workers (when the API provides them).</p>
        <div id="jobDetailLogsBody" class="job-detail-logs-body"></div>
      </div>
    </div>
  </div>`;
}

function escapeHtml(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Syntax-highlight JSON keys for display (safe HTML). */
function highlightJsonToHtml(jsonStr) {
  const s = String(jsonStr);
  let out = "";
  const re = /(^|[{[,\n])(\s*)("(?:\\.|[^"\\])*")\s*:/gm;
  let last = 0;
  let m;
  while ((m = re.exec(s)) !== null) {
    const keyStart = m.index + m[1].length + m[2].length;
    const keyEnd = keyStart + m[3].length;
    const afterKey = m.index + m[0].length;
    out += escapeHtml(s.slice(last, keyStart));
    out += `<span class="job-json-key">${escapeHtml(m[3])}</span>`;
    out += escapeHtml(s.slice(keyEnd, afterKey));
    last = afterKey;
  }
  out += escapeHtml(s.slice(last));
  return out;
}

function copyJobDetailLink() {
  const q = detailJob?.queue_name ?? parseJobRoute(window.location.hash)?.queueName;
  const id = detailJob?.id ?? currentJobId ?? parseJobRoute(window.location.hash)?.jobId;
  if (!q || !id) return;
  const url =
    window.location.origin +
    window.location.pathname +
    window.location.search +
    jobRouteHash(q, String(id));
  void navigator.clipboard.writeText(url).catch(() => {
    prompt("Copy this URL:", url);
  });
}

function closeJobDetailPage(opts = {}) {
  if (!jobDetailActive) return;

  jobDetailActive = false;
  currentJobId = null;
  detailJob = null;

  stopJobDetailRelativeTimer();
  hideJobDetailToolbarMeta();
  jobDetailLastFetchedAt = null;
  jobDetailCloseOverflowMenu();

  document.getElementById("job-detail-view").style.display = "none";

  const retryBtn = document.getElementById("retryJobBtn");
  if (retryBtn) retryBtn.style.display = "none";

  if (!opts.skipHashClear) {
    clearHashRoute();
  }

  if (currentQueue && queues.includes(currentQueue)) {
    document.getElementById("empty-state").style.display = "none";
    document.getElementById("queue-view").style.display = "block";
    document.querySelectorAll(".queue-item").forEach((item) => {
      item.classList.toggle("active", item.dataset.queue === currentQueue);
    });
    loadQueueStats();
    loadJobs();
  } else if (queues.length > 0) {
    selectQueue(queues[0]);
  } else {
    document.getElementById("empty-state").style.display = "block";
    document.getElementById("queue-view").style.display = "none";
    currentQueue = "";
  }
}

function retryJobById(jobId) {
  currentJobId = jobId;
  retryJob();
}

function deleteJobById(jobId, queueName) {
  if (!confirm("Are you sure you want to delete this job?")) {
    return;
  }
  currentJobId = jobId;
  deleteJob(queueName);
}

async function deleteJobApi(jobId, queueName) {
  const params = new URLSearchParams({ queue_name: queueName });
  const response = await fetch(
    `${API_BASE}/jobs/${encodeURIComponent(jobId)}/delete?${params}`,
    { method: "DELETE" },
  );
  const data = await response.json().catch(() => ({}));
  return { response, data };
}

async function deleteSelectedJobs() {
  const records = getSelectedJobRecords();
  if (records.length === 0) return;

  if (
    !confirm(
      `Delete ${records.length} job${records.length === 1 ? "" : "s"}? This cannot be undone.`,
    )
  ) {
    return;
  }

  let failures = 0;
  for (const job of records) {
    const { response } = await deleteJobApi(job.id, job.queue_name);
    if (response.ok) {
      selectedJobIds.delete(job.id);
    } else {
      failures++;
    }
  }

  if (failures > 0) {
    alert(
      `${failures} job${failures === 1 ? "" : "s"} could not be deleted. The rest were removed.`,
    );
  }

  loadQueueStats();
  await loadJobs();
}

async function retrySelectedJobs() {
  const records = getSelectedJobRecords().filter((j) => j.state === "Failed");
  if (records.length === 0) return;

  if (
    !confirm(
      `Retry ${records.length} failed job${records.length === 1 ? "" : "s"}?`,
    )
  ) {
    return;
  }

  let failures = 0;
  for (const job of records) {
    const response = await fetch(`${API_BASE}/jobs/${job.id}/retry`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ queue_name: job.queue_name }),
    });
    if (!response.ok) {
      failures++;
    }
  }

  if (failures > 0) {
    alert(
      `${failures} job${failures === 1 ? "" : "s"} could not be retried.`,
    );
  }

  clearJobSelection();
  loadQueueStats();
  await loadJobs();
}

async function retryJob() {
  if (!currentJobId) return;
  const queueName = detailJob?.queue_name ?? currentQueue;
  if (!queueName) return;

  try {
    const response = await fetch(`${API_BASE}/jobs/${currentJobId}/retry`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ queue_name: queueName }),
    });

    const data = await response.json();

    if (response.ok) {
      loadQueueStats();
      loadJobs();
      if (jobDetailActive) {
        await loadJobDetailPageContent({ silent: true });
      }
    } else {
      alert("Failed to retry job: " + (data.error || "Unknown error"));
    }
  } catch (error) {
    alert("Failed to retry job: " + error.message);
  }
}

async function deleteJob(queueNameOverride) {
  const queueName =
    queueNameOverride ?? detailJob?.queue_name ?? currentQueue;
  if (!currentJobId || !queueName) {
    alert(
      !currentJobId
        ? "No job selected."
        : "No queue context for this job. Choose a queue in the sidebar or use the table Delete button.",
    );
    return;
  }

  try {
    const { response, data } = await deleteJobApi(currentJobId, queueName);

    if (response.ok) {
      selectedJobIds.delete(currentJobId);
      if (jobDetailActive) {
        closeJobDetailPage();
      } else {
        loadQueueStats();
        await loadJobs();
      }
    } else {
      alert("Failed to delete job: " + (data.error || "Unknown error"));
    }
  } catch (error) {
    alert("Failed to delete job: " + error.message);
  }
}

async function processDelayed() {
  if (!currentQueue) return;

  if (
    !confirm(
      `Process delayed jobs from ${currentQueue}? This will move delayed jobs that are due to the waiting queue.`,
    )
  ) {
    return;
  }

  try {
    const response = await fetch(
      `${API_BASE}/queues/${currentQueue}/process-delayed`,
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
      },
    );

    const data = await response.json();

    if (response.ok) {
      alert(
        `Successfully moved ${data.moved_count || 0} delayed jobs to waiting`,
      );
      loadQueueStats();
      loadJobs();
    } else {
      alert(
        "Failed to process delayed jobs: " + (data.error || "Unknown error"),
      );
    }
  } catch (error) {
    alert("Failed to process delayed jobs: " + error.message);
  }
}

async function recoverStalled() {
  if (!currentQueue) return;

  if (
    !confirm(
      `Recover stalled jobs from ${currentQueue}? This will move jobs that have been active too long back to the waiting queue.`,
    )
  ) {
    return;
  }

  try {
    const response = await fetch(
      `${API_BASE}/queues/${currentQueue}/recover-stalled`,
      {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
      },
    );

    const data = await response.json();

    if (response.ok) {
      alert(`Successfully recovered ${data.recovered_count || 0} stalled jobs`);
      loadQueueStats();
      loadJobs();
    } else {
      alert(
        "Failed to recover stalled jobs: " + (data.error || "Unknown error"),
      );
    }
  } catch (error) {
    alert("Failed to recover stalled jobs: " + error.message);
  }
}

async function cleanQueue() {
  if (!currentQueue) return;

  const state = prompt(
    'Enter state to clean (waiting, delayed, failed, completed, or "all"):\n\nNote: Active jobs cannot be cleaned.',
  );
  if (!state) return;

  if (state.toLowerCase() === "active") {
    alert("Cannot clean active jobs. They are currently being processed.");
    return;
  }

  if (
    !confirm(
      `Are you sure you want to clean ${state} jobs from ${currentQueue}? This action cannot be undone.`,
    )
  ) {
    return;
  }

  try {
    const response = await fetch(`${API_BASE}/queues/clean`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        queue_name: currentQueue,
        state: state || "all",
      }),
    });

    const data = await response.json();

    if (response.ok) {
      alert(`Successfully cleaned ${data.deleted_count || 0} jobs`);
      loadQueueStats();
      loadJobs();
    } else {
      alert("Failed to clean queue: " + (data.error || "Unknown error"));
    }
  } catch (error) {
    alert("Failed to clean queue: " + error.message);
  }
}

// Make functions available globally for onclick handlers
window.retryJobById = retryJobById;
window.deleteJobById = deleteJobById;
