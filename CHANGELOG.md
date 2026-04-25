# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.3.1] - 2026-04-24

### Added

- `**ChainMQError::InvalidJobId`:** returned when a custom job id is empty on enqueue, or when an empty id appears on the wait stream during claim.
- **Integration test:** `enqueue_custom_string_job_id_claim_roundtrip` (Redis-ignored) covers non-UUID custom ids end-to-end.

### Changed

- `**JobId`:** now wraps any non-empty string (serde transparent JSON string); auto-generated ids are still UUID strings. Queue operations (`claim_job`, `list_jobs`, `recover_stalled_jobs`, delayed promotion), dashboard job routes, and the job-log tracing layer accept arbitrary string ids. `**JobOptions::job_id`** documents custom string ids.
- **Web UI (`ui/app.js`):** activity “Logged at” / schedule copy, job list created/execute/start times, lifecycle milestone labels, and related helpers display **fractional seconds** (three-digit milliseconds) via `Intl` / `toLocaleString` options.
- `**examples/enqueue_email.rs`:** no longer calls `complete_job` immediately after enqueueing a delayed job (that path skipped `delay_secs` and `perform`); added a short note to run a worker for real delayed execution.

## [1.3.0] - 2026-04-23

### Added

- **Queue lifecycle events:** `Queue::emit_queue_event` appends JSON to `{key_prefix}:events:{queue_name}:stream` (`XADD` with `MAXLEN ~` from `QueueOptions::events_stream_max_len`) and `PUBLISH`es the same payload on `{key_prefix}:events:{queue_name}`; event kinds include `waiting`, `delayed`, `active`, `progress`, `completed`, `failed`, `stalled`, `delayed_moved`, `removed`, `retried`.
- `**Queue::read_queue_events` / `redis_server_metrics_json`:** helpers for the dashboard (`INFO` whitelist).
- **Web UI:** `GET …/api/queues/{queue}/events`, `GET …/api/queues/{queue}/events/live` (SSE when Redis is URL/Client), `GET …/api/redis/stats`; job **Activity** tab and **Redis** toolbar modal for server memory / `INFO`.
- `**JobContext::queue`:** `perform` receives the same `Arc<Queue>` the worker uses.
- `**JobContext::set_progress`:** async persistence of `JobMetadata.progress` (JSON), surfaced on the dashboard job detail view.
- `**JobContext::cancellation_token`:** cooperative cancellation via `tokio_util::sync::CancellationToken`; worker cancels parent token on graceful or force shutdown and requeues the job if cancellation wins the run race.
- `**JobOptions::lifo`:** per-job LIFO wait bucket (`waitl:p`*) vs FIFO (`wait:p`*); claim order still prefers higher priority first.
- `**JobOptions::priority`:** enforced on claim (per-priority FIFO/LIFO lists plus legacy `{prefix}:queue:{name}:wait` drained last for migration).
- `**QueueOptions::max_completed_len` / `max_failed_len`:** optional list retention after completion or terminal failure (defaults preserve prior completed trim behavior when set).
- **Re-export:** `CancellationToken` from the crate root for convenience.

### Changed

- **Delayed moves:** `[move_delayed.lua](src/lua/move_delayed.lua)` no longer publishes queue events; Rust emits one `delayed_moved` event per promoted job after the script returns.
- **Wait queue layout:** jobs are pushed to `…:wait:p{1|5|10|20}` or `…:waitl:p{…}`; `[move_delayed.lua](src/lua/move_delayed.lua)` reads `priority` / `enqueue_lifo` hash fields on the job key when promoting delayed jobs.
- `**claim_job` Lua script:** accepts a variable ordered list of wait keys ending with the active set key.

### Breaking

- **Worker tracing install:** `Worker` no longer installs a default tracing subscriber with Redis job logs unless `**WorkerConfig::tracing_job_logs`** is `true` (or `**WorkerBuilder::with_tracing_job_logs(true)`**). Queue lifecycle events are the default observability path; opt in to retain the old dual stdout + Redis log behavior.
- `**JobContext::new`:** now requires `Arc<Queue>` and `CancellationToken` in addition to job id, metadata, and app context. Call sites that constructed `JobContext` outside the worker must be updated (the worker supplies both values).

### Dependencies

- Added `**tokio-util`** (for `CancellationToken`).

