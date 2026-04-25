// examples/schedulers/repeat_and_pause.rs
//
// End-to-end demo: interval + cron repeatables, `process_repeat`, queue pause/resume,
// a real [`Worker`] (shared [`Queue`], fast poll intervals), optional Axum dashboard, and cleanup.
//
// ## Prerequisites
// - Redis (default `redis://localhost:6370`, same as other examples).
//
// ## Environment
// - `REDIS_URL` — override Redis URL.
// - `CHAINMQ_DEMO_SKIP_UI=1` — do not bind the dashboard (useful in headless environments).
// - `CHAINMQ_DEMO_UI_ADDR` — bind address for the dashboard (default `127.0.0.1:8099`).
//
// ## Flow (about 25–30 seconds)
// 1. Tear down any prior demo repeats on this queue; ensure the queue is not paused.
// 2. Start a worker in the background (claims jobs, runs `process_repeat` on an interval).
// 3. Register an **interval** repeat (immediate first fire, 10s period) and watch jobs run.
// 4. Call **`process_repeat`** explicitly (often 0 if the worker already promoted due ticks).
// 5. Register a **cron** repeat (six-field expression with seconds; here every 7s) and observe more runs.
// 6. **`pause_queue`**: repeat promotion stops; **claims** stop; enqueue still works.
// 7. **`resume_queue`**: drain the manual job, then remove both schedules and stop background tasks.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::Router;
use chainmq::{
    AppContext, Job, JobContext, JobOptions, JobRegistry, JobState, Priority, Queue, QueueOptions,
    RedisClient, RepeatCatchUp, Result, WebUIMountConfig, WorkerBuilder, async_trait,
    chainmq_dashboard_router, serde_json::json,
};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

const QUEUE_NAME: &str = "repeat_demo";
const INTERVAL_SCHEDULE_ID: &str = "demo-interval";
const CRON_SCHEDULE_ID: &str = "demo-cron";

#[derive(Debug, Serialize, Deserialize)]
struct DemoJob {
    /// Where this run came from: `interval`, `cron`, or `manual`.
    source: String,
    seq: u32,
}

#[derive(Clone)]
struct DemoApp {
    /// Successful `perform` invocations (for the scripted demo).
    completed: Arc<AtomicU64>,
}

impl DemoApp {
    fn new() -> Self {
        Self {
            completed: Arc::new(AtomicU64::new(0)),
        }
    }

    fn performed(&self) -> u64 {
        self.completed.load(Ordering::SeqCst)
    }

    fn record(&self) -> u64 {
        self.completed.fetch_add(1, Ordering::SeqCst) + 1
    }
}

impl AppContext for DemoApp {
    fn clone_context(&self) -> Arc<dyn AppContext> {
        Arc::new(Self {
            completed: Arc::clone(&self.completed),
        })
    }
}

#[async_trait]
impl Job for DemoJob {
    async fn perform(&self, ctx: &JobContext) -> Result<()> {
        let n = ctx
            .app::<DemoApp>()
            .map(|a| a.record())
            .unwrap_or_default();
        println!(
            "[DemoJob] run #{n} job_id={} source={} seq={}",
            ctx.job_id, self.source, self.seq
        );
        ctx.set_response(json!({
            "source": self.source,
            "seq": self.seq,
            "demo_run": n,
        }));
        Ok(())
    }

    fn name() -> &'static str {
        "DemoJob"
    }

    fn queue_name() -> &'static str {
        QUEUE_NAME
    }
}

fn build_registry() -> JobRegistry {
    let mut r = JobRegistry::new();
    r.register::<DemoJob>();
    r
}

fn queue_options(redis_url: &str) -> QueueOptions {
    QueueOptions {
        redis: RedisClient::Url(redis_url.to_string()),
        ..Default::default()
    }
}

async fn reset_demo_state(queue: &Queue) -> anyhow::Result<()> {
    if queue.is_queue_paused(QUEUE_NAME).await? {
        queue.resume_queue(QUEUE_NAME).await?;
    }
    for row in queue.list_repeats(QUEUE_NAME).await? {
        queue
            .remove_repeat(QUEUE_NAME, &row.schedule_id)
            .await
            .ok();
    }
    Ok(())
}

async fn print_snapshot(queue: &Queue, label: &str) -> anyhow::Result<()> {
    let stats = queue.get_stats(QUEUE_NAME).await?;
    let paused = queue.is_queue_paused(QUEUE_NAME).await?;
    println!(
        "  [{label}] paused={paused} waiting={} active={} delayed={} completed={} failed={}",
        stats.waiting,
        stats.active,
        stats.delayed,
        stats.completed,
        stats.failed
    );
    Ok(())
}

fn spawn_worker(
    redis_url: String,
    queue: Arc<Queue>,
    app: Arc<DemoApp>,
) -> JoinHandle<()> {
    let registry = build_registry();
    tokio::spawn(async move {
        let client = match redis::Client::open(redis_url.as_str()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[demo] redis client open failed: {e}");
                return;
            }
        };
        let mut worker = match WorkerBuilder::new_with_redis_instance(&client, registry)
            .with_shared_queue(queue)
            .with_app_context(app)
            .with_queue_name(QUEUE_NAME)
            .with_poll_interval(Duration::from_millis(200))
            .with_repeat_poll_interval(Duration::from_millis(700))
            .with_concurrency(2)
            .spawn()
            .await
        {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[demo] worker spawn failed: {e}");
                return;
            }
        };
        if let Err(e) = worker.start().await {
            eprintln!("[demo] worker exited with error: {e}");
        }
    })
}

async fn maybe_spawn_dashboard(
    redis_url: String,
) -> anyhow::Result<Option<JoinHandle<()>>> {
    if std::env::var("CHAINMQ_DEMO_SKIP_UI").ok().as_deref() == Some("1") {
        println!("\n[demo] CHAINMQ_DEMO_SKIP_UI=1 — not starting the dashboard.");
        return Ok(None);
    }

    let bind = std::env::var("CHAINMQ_DEMO_UI_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8099".to_string());
    let addr: SocketAddr = bind.parse().map_err(|e| {
        anyhow::anyhow!("CHAINMQ_DEMO_UI_ADDR={bind:?} is not a valid socket address: {e}")
    })?;

    let queue_ui = Queue::new(queue_options(&redis_url))
        .await
        .map_err(|e| anyhow::anyhow!("dashboard Queue::new: {e}"))?;
    let mount = WebUIMountConfig {
        ui_path: "/dashboard".to_string(),
        auth: None,
        ..Default::default()
    };
    let dashboard = chainmq_dashboard_router(queue_ui, mount)
        .map_err(|e| anyhow::anyhow!("chainmq_dashboard_router: {e}"))?;
    let app = Router::new().nest_service("/dashboard", dashboard);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "[demo] could not bind dashboard at {addr} ({e}); continuing without UI."
            );
            return Ok(None);
        }
    };

    let local = listener
        .local_addr()
        .map_err(|e| anyhow::anyhow!("local_addr: {e}"))?;
    println!(
        "\n[demo] Dashboard (no auth): http://{local}/dashboard/ — queue `{QUEUE_NAME}`, login disabled for this demo."
    );

    Ok(Some(tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    })))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::try_init().ok();

    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://localhost:6370".to_string());

    println!(
        "\n=== ChainMQ repeatables + pause demo ===\nRedis: {redis_url}\nLogical queue: `{QUEUE_NAME}`"
    );

    let queue = Arc::new(
        Queue::new(queue_options(&redis_url))
            .await
            .map_err(|e| anyhow::anyhow!("Queue::new: {e}"))?,
    );

    reset_demo_state(&queue).await?;
    print_snapshot(&queue, "after reset").await?;

    let app = Arc::new(DemoApp::new());
    let worker_handle = spawn_worker(redis_url.clone(), Arc::clone(&queue), Arc::clone(&app));
    let ui_handle = maybe_spawn_dashboard(redis_url.clone()).await?;

    // Let the worker attach before we add schedules.
    tokio::time::sleep(Duration::from_millis(600)).await;

    println!("\n--- 1) Interval repeat: first fire now, then every 10s (catch-up: one) ---");
    queue
        .upsert_repeat_interval(
            QUEUE_NAME,
            INTERVAL_SCHEDULE_ID,
            DemoJob::name(),
            json!({ "source": "interval", "seq": 1 }),
            JobOptions::default(),
            10,
            true,
            RepeatCatchUp::One,
            Some(0),
        )
        .await?;

    for info in queue.list_repeats(QUEUE_NAME).await? {
        println!(
            "  repeat: id={} enabled={} interval_secs={:?} cron={:?} next_run_unix={}",
            info.schedule_id,
            info.enabled,
            info.interval_secs,
            info.cron_expr,
            info.next_run_unix
        );
    }

    print_snapshot(&queue, "after interval upsert").await?;
    println!(
        "  (watch for [DemoJob] lines from the worker; completed counter = {})",
        app.performed()
    );
    tokio::time::sleep(Duration::from_secs(4)).await;
    println!(
        "  after ~4s: completed counter = {} (interval tick(s) should have run)",
        app.performed()
    );

    println!("\n--- 2) Explicit process_repeat (extra promotion pass from this process) ---");
    let extra = queue
        .process_repeat(QUEUE_NAME)
        .await
        .map_err(|e| anyhow::anyhow!("process_repeat: {e}"))?;
    println!("  process_repeat returned {extra} new job(s) this call.");
    print_snapshot(&queue, "after explicit process_repeat").await?;

    println!("\n--- 3) Cron repeat: every 7 seconds (six-field cron, seconds field first) ---");
    queue
        .upsert_repeat_cron(
            QUEUE_NAME,
            CRON_SCHEDULE_ID,
            DemoJob::name(),
            "*/7 * * * * *",
            json!({ "source": "cron", "seq": 0 }),
            JobOptions::default(),
            true,
        )
        .await
        .map_err(|e| anyhow::anyhow!("upsert_repeat_cron: {e}"))?;

    tokio::time::sleep(Duration::from_secs(9)).await;
    println!(
        "  after ~9s with interval + cron: completed counter = {}",
        app.performed()
    );
    print_snapshot(&queue, "with both schedules").await?;

    println!("\n--- 4) Pause: no new repeat promotions; claims blocked; enqueue still works ---");
    queue.pause_queue(QUEUE_NAME).await?;
    let counter_at_pause = app.performed();
    let promoted_while_paused = queue
        .process_repeat(QUEUE_NAME)
        .await?;
    println!(
        "  process_repeat while paused -> {promoted_while_paused} (expected 0); paused={}",
        queue.is_queue_paused(QUEUE_NAME).await?
    );

    tokio::time::sleep(Duration::from_secs(3)).await;
    println!(
        "  after 3s paused: completed counter = {} (baseline {} — no new repeat promotions; in-flight jobs may still finish)",
        app.performed(),
        counter_at_pause
    );

    let manual_opts = JobOptions {
        priority: Priority::High,
        ..Default::default()
    };
    let manual_id = queue
        .enqueue_with_options(
            DemoJob {
                source: "manual".into(),
                seq: 42,
            },
            manual_opts,
        )
        .await?;
    println!("  enqueued high-priority manual job while paused: {manual_id}");
    print_snapshot(&queue, "paused + manual in wait").await?;

    println!("\n--- 5) Resume: worker claims the queued manual job and repeat ticks resume ---");
    queue.resume_queue(QUEUE_NAME).await?;

    tokio::time::sleep(Duration::from_secs(5)).await;
    println!(
        "  after resume ~5s: completed counter = {} (includes manual + any due repeats)",
        app.performed()
    );
    print_snapshot(&queue, "after resume window").await?;

    let waiting = queue
        .list_jobs(QUEUE_NAME, JobState::Waiting, Some(15))
        .await?;
    println!("  sample waiting job names: {:?}", waiting.iter().map(|m| &m.name).collect::<Vec<_>>());

    println!("\n--- 6) Cleanup: remove repeats (worker may still process in-flight jobs) ---");
    queue
        .remove_repeat(QUEUE_NAME, INTERVAL_SCHEDULE_ID)
        .await?;
    queue.remove_repeat(QUEUE_NAME, CRON_SCHEDULE_ID).await?;
    print_snapshot(&queue, "after remove_repeat").await?;

    if queue.list_repeats(QUEUE_NAME).await?.is_empty() {
        println!("  repeat ZSET/HASH for this queue cleared.");
    }

    println!("\n=== Demo script finished; stopping background tasks ===");
    if let Some(h) = ui_handle {
        h.abort();
    }
    worker_handle.abort();

    println!(
        "\nTip: in your own app, use `WorkerBuilder::with_repeat_poll_interval`, \
`Queue::process_repeat` from a cron sidecar; dashboard process-repeat is queue-native."
    );

    Ok(())
}
