# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project aims to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.2.1] - 2026-04-23

### Added

- **`JobContext::queue`:** `perform` receives the same `Arc<Queue>` the worker uses (BullMQ-style `job.queue`).
- **`JobContext::set_progress`:** async persistence of `JobMetadata.progress` (JSON), surfaced on the dashboard job detail view.
- **`JobContext::cancellation_token`:** cooperative cancellation via `tokio_util::sync::CancellationToken`; worker cancels parent token on graceful or force shutdown and requeues the job if cancellation wins the run race.
- **`JobOptions::lifo`:** per-job LIFO wait bucket (`waitl:p*`) vs FIFO (`wait:p*`); claim order still prefers higher priority first.
- **`JobOptions::priority`:** enforced on claim (per-priority FIFO/LIFO lists plus legacy `{prefix}:queue:{name}:wait` drained last for migration).
- **`QueueOptions::max_completed_len` / `max_failed_len`:** optional list retention after completion or terminal failure (defaults preserve prior completed trim behavior when set).
- **Redis pub/sub:** `{key_prefix}:events:{queue_name}` JSON events for `completed` and `failed` (delayed moves already published `delayed_moved`).
- **Re-export:** `CancellationToken` from the crate root for convenience.

### Changed

- **Wait queue layout:** jobs are pushed to `…:wait:p{1|5|10|20}` or `…:waitl:p{…}`; [`move_delayed.lua`](src/lua/move_delayed.lua) reads `priority` / `enqueue_lifo` hash fields on the job key when promoting delayed jobs.
- **`claim_job` Lua script:** accepts a variable ordered list of wait keys ending with the active set key.

### Breaking

- **`JobContext::new`:** now requires `Arc<Queue>` and `CancellationToken` in addition to job id, metadata, and app context. Call sites that constructed `JobContext` outside the worker must be updated (the worker supplies both values).

### Dependencies

- Added **`tokio-util`** (for `CancellationToken`).
