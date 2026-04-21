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

let queues = [];
let currentQueue = "";
let currentState = "waiting";
let currentJobId = null;
let currentPage = 1;
let pageSize = 25;
let searchQuery = "";
let allJobs = [];

// Theme management
function initTheme() {
  const savedTheme = localStorage.getItem("theme") || "light";
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

  // Auto-refresh every 3 seconds
  setInterval(() => {
    if (currentQueue) {
      loadQueueStats();
      loadJobs();
    } else {
      loadQueues();
    }
  }, 3000);
});

function setupEventListeners() {
  // Theme toggle
  document.getElementById("themeToggle").addEventListener("click", toggleTheme);

  // Refresh button
  document.getElementById("refreshBtn").addEventListener("click", () => {
    if (currentQueue) {
      loadQueueStats();
      loadJobs();
    } else {
      loadQueues();
    }
  });

  // Stat cards - make them clickable
  document.querySelectorAll(".stat-card").forEach((card) => {
    card.addEventListener("click", () => {
      const state = card.dataset.state;
      if (state && currentQueue) {
        switchState(state);
      }
    });
  });

  // Tab buttons
  document.querySelectorAll(".tab-btn").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      const state = e.currentTarget.dataset.state;
      switchState(state);
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
  document.getElementById("deleteJobBtn").addEventListener("click", deleteJob);

  // Modal close on outside click
  document.getElementById("jobModal").addEventListener("click", (e) => {
    if (e.target.id === "jobModal") {
      closeJobModal();
    }
  });
}

function switchState(state) {
  currentState = state;
  currentPage = 1;

  // Update active tab
  document.querySelectorAll(".tab-btn").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.state === state);
  });

  // Update active stat card
  document.querySelectorAll(".stat-card").forEach((card) => {
    card.classList.toggle("active", card.dataset.state === state);
  });

  loadJobs();
}

async function loadQueues() {
  try {
    const response = await fetch(`${API_BASE}/queues`);
    const data = await response.json();

    queues = data.queues || [];
    renderQueues();

    // Auto-select first queue if none selected
    if (!currentQueue && queues.length > 0) {
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

  // Add click listeners
  queuesList.querySelectorAll(".queue-item").forEach((item) => {
    item.addEventListener("click", () => {
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
  currentQueue = queueName;
  currentState = "waiting";
  currentPage = 1;
  searchQuery = "";
  document.getElementById("searchInput").value = "";

  // Update UI
  document.getElementById("empty-state").style.display = "none";
  document.getElementById("queue-view").style.display = "block";
  document.getElementById("queue-name-display").textContent = queueName;

  // Update active queue in sidebar
  document.querySelectorAll(".queue-item").forEach((item) => {
    item.classList.toggle("active", item.dataset.queue === queueName);
  });

  // Reset tabs
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

    // Update tab counts
    document.getElementById("tab-waiting").textContent = data.waiting || 0;
    document.getElementById("tab-active").textContent = data.active || 0;
    document.getElementById("tab-delayed").textContent = data.delayed || 0;
    document.getElementById("tab-failed").textContent = data.failed || 0;
    document.getElementById("tab-completed").textContent = data.completed || 0;

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
      `${API_BASE}/queues/${currentQueue}/jobs/${currentState}?limit=1000`
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
      } ${currentState} jobs for queue ${currentQueue}`
    );
    console.log("Jobs data:", data.jobs?.slice(0, 2)); // Debug: show first 2 jobs
    allJobs = data.jobs || [];
    renderJobs();
  } catch (error) {
    console.error("Failed to load jobs:", error);
    document.getElementById(
      "jobs-table-body"
    ).innerHTML = `<tr><td colspan="6" class="loading-jobs">Failed to load jobs: ${error.message}</td></tr>`;
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
        <td colspan="6" class="empty-jobs">
          <div class="empty-jobs-icon">📭</div>
          <div>No ${currentState} jobs found</div>
        </td>
      </tr>
    `;
    document.getElementById("pagination").style.display = "none";
    return;
  }

  tbody.innerHTML = pageJobs.map((job) => createJobRow(job)).join("");

  // Add click listeners for view details
  tbody.querySelectorAll(".job-row").forEach((row) => {
    row.addEventListener("click", (e) => {
      if (!e.target.closest(".btn")) {
        const jobId = row.dataset.jobId;
        showJobDetails(jobId);
      }
    });
  });

  // Update pagination
  document.getElementById("pageInfo").textContent = `Page ${currentPage} of ${
    totalPages || 1
  }`;
  document.getElementById("prevPage").disabled = currentPage === 1;
  document.getElementById("nextPage").disabled = currentPage >= totalPages;
  document.getElementById("pagination").style.display =
    totalPages > 1 ? "flex" : "none";
}

function createJobRow(job) {
  const created = new Date(job.created_at).toLocaleString();
  const stateClass = job.state.toLowerCase();

  // For delayed jobs, show when they'll execute
  let timeInfo = created;
  if (job.state === "Delayed" && job.options.delay_secs) {
    const executeAt = new Date(
      new Date(job.created_at).getTime() + job.options.delay_secs * 1000
    );
    timeInfo = `<div>${created}</div><div style="font-size: 12px; color: var(--text-secondary); margin-top: 4px;">Executes: ${executeAt.toLocaleString()}</div>`;
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

  return `
    <tr class="job-row" data-job-id="${job.id}">
      <td><span class="job-id">${job.id.substring(0, 8)}...</span></td>
      <td><span class="job-name">${job.name}</span></td>
      <td><span class="job-state-badge ${stateClass}">${job.state}</span></td>
      <td>${timeInfo}</td>
      <td>${job.attempts} / ${job.options.attempts}</td>
      <td>
        <div class="job-actions">
          ${
            job.state === "Failed"
              ? '<button class="btn btn-success btn-sm" onclick="event.stopPropagation(); retryJobById(\'' +
                job.id +
                "')\">Retry</button>"
              : ""
          }
          <button class="btn btn-danger btn-sm" onclick="event.stopPropagation(); deleteJobById(\'' + job.id + '\')">Delete</button>
        </div>
      </td>
    </tr>
  `;
}

async function showJobDetails(jobId) {
  try {
    currentJobId = jobId;
    const response = await fetch(`${API_BASE}/jobs/${jobId}`);
    const job = await response.json();

    const modal = document.getElementById("jobModal");
    const details = document.getElementById("jobDetails");
    const retryBtn = document.getElementById("retryJobBtn");

    details.innerHTML = `
      <div class="job-detail">
        <pre>${JSON.stringify(job, null, 2)}</pre>
      </div>
    `;

    // Show retry button only for failed jobs
    retryBtn.style.display = job.state === "Failed" ? "block" : "none";

    modal.style.display = "flex";
  } catch (error) {
    alert("Failed to load job details: " + error.message);
  }
}

function closeJobModal() {
  document.getElementById("jobModal").style.display = "none";
  currentJobId = null;
}

function retryJobById(jobId) {
  currentJobId = jobId;
  retryJob();
}

function deleteJobById(jobId) {
  if (!confirm("Are you sure you want to delete this job?")) {
    return;
  }
  currentJobId = jobId;
  deleteJob();
}

async function retryJob() {
  if (!currentJobId || !currentQueue) return;

  try {
    const response = await fetch(`${API_BASE}/jobs/${currentJobId}/retry`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ queue_name: currentQueue }),
    });

    const data = await response.json();

    if (response.ok) {
      closeJobModal();
      loadQueueStats();
      loadJobs();
    } else {
      alert("Failed to retry job: " + (data.error || "Unknown error"));
    }
  } catch (error) {
    alert("Failed to retry job: " + error.message);
  }
}

async function deleteJob() {
  if (!currentJobId || !currentQueue) return;

  try {
    const response = await fetch(`${API_BASE}/jobs/${currentJobId}/delete`, {
      method: "DELETE",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ queue_name: currentQueue }),
    });

    const data = await response.json();

    if (response.ok) {
      closeJobModal();
      loadQueueStats();
      loadJobs();
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
      `Process delayed jobs from ${currentQueue}? This will move delayed jobs that are due to the waiting queue.`
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
      }
    );

    const data = await response.json();

    if (response.ok) {
      alert(
        `Successfully moved ${data.moved_count || 0} delayed jobs to waiting`
      );
      loadQueueStats();
      loadJobs();
    } else {
      alert(
        "Failed to process delayed jobs: " + (data.error || "Unknown error")
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
      `Recover stalled jobs from ${currentQueue}? This will move jobs that have been active too long back to the waiting queue.`
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
      }
    );

    const data = await response.json();

    if (response.ok) {
      alert(`Successfully recovered ${data.recovered_count || 0} stalled jobs`);
      loadQueueStats();
      loadJobs();
    } else {
      alert(
        "Failed to recover stalled jobs: " + (data.error || "Unknown error")
      );
    }
  } catch (error) {
    alert("Failed to recover stalled jobs: " + error.message);
  }
}

async function cleanQueue() {
  if (!currentQueue) return;

  const state = prompt(
    'Enter state to clean (waiting, delayed, failed, completed, or "all"):\n\nNote: Active jobs cannot be cleaned.'
  );
  if (!state) return;

  if (state.toLowerCase() === "active") {
    alert("Cannot clean active jobs. They are currently being processed.");
    return;
  }

  if (
    !confirm(
      `Are you sure you want to clean ${state} jobs from ${currentQueue}? This action cannot be undone.`
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
window.closeJobModal = closeJobModal;
