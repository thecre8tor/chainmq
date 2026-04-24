# ChainMQ vs BullMQ — concept map

This project is inspired by [BullMQ](https://docs.bullmq.io/) (queues, workers, jobs). Use this table to find the closest ChainMQ API or artefact. Status is approximate; see crate docs for exact behaviour.

| BullMQ | ChainMQ | Status |
|--------|---------|--------|
| `Queue`, `queue.add(name, data, opts)` | [`Queue::enqueue`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html#method.enqueue), [`enqueue_with_options`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html#method.enqueue_with_options) | Done |
| Logical queue name (string) | [`Job::queue_name()`](https://docs.rs/chainmq/latest/chainmq/trait.Job.html#tymethod.queue_name) (type-level) | Different model |
| `Worker` + processor function | [`Worker`](https://docs.rs/chainmq/latest/chainmq/struct.Worker.html) + [`Job::perform`](https://docs.rs/chainmq/latest/chainmq/trait.Job.html#tymethod.perform) | Done |
| `job.queue` on the job handle | [`JobContext::queue`](https://docs.rs/chainmq/latest/chainmq/struct.JobContext.html#method.queue) → `Arc<Queue>` | Done |
| `job.data` / `job.opts` | [`JobContext::job_metadata`](https://docs.rs/chainmq/latest/chainmq/struct.JobContext.html) (`payload`, `options`) | Done |
| `job.updateProgress` | [`JobContext::set_progress`](https://docs.rs/chainmq/latest/chainmq/struct.JobContext.html#method.set_progress) | Done |
| `QueueEvents` (Redis pub/sub) | Same Redis `PUBLISH` pattern as delayed moves: `{key_prefix}:events:{queue_name}` with JSON payloads (`completed`, `failed`, `delayed_moved`, …) | Partial (see events payload in Redis) |
| `job.retry()` (failed → wait) | [`Queue::retry_job`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html#method.retry_job) or dashboard retry | Done |
| Retry after processor error | Worker calls [`Queue::fail_after_perform_error`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html#method.fail_after_perform_error) (delayed retry or terminal failed) | Done |
| Delayed jobs | [`JobOptions::delay_secs`](https://docs.rs/chainmq/latest/chainmq/struct.JobOptions.html) + delayed sorted set | Done |
| Priority on wait | [`JobOptions::priority`](https://docs.rs/chainmq/latest/chainmq/struct.JobOptions.html) + per-priority wait buckets | Done |
| LIFO per job | [`JobOptions::lifo`](https://docs.rs/chainmq/latest/chainmq/struct.JobOptions.html) | Done |
| Remove-on-complete / retention | [`QueueOptions::max_completed_len`](https://docs.rs/chainmq/latest/chainmq/struct.QueueOptions.html), [`max_failed_len`](https://docs.rs/chainmq/latest/chainmq/struct.QueueOptions.html) | Done |
| Cooperative cancellation (`AbortSignal`) | [`JobContext::cancellation_token`](https://docs.rs/chainmq/latest/chainmq/struct.JobContext.html#method.cancellation_token) (`tokio_util::sync::CancellationToken`) | Done |
| Repeatable / cron jobs | Not implemented | See [repeatables / cron design](repeatables-cron-design.md) |

## Semantics that differ on purpose

- **Retry vs re-enqueue:** Dashboard or [`retry_job`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html#method.retry_job) moves a failed job back to **waiting** and the worker runs **`perform` again** — it does **not** run producer `enqueue` logic again. Same idea as BullMQ re-running the processor when a failed job is retried.
- **Typed jobs:** ChainMQ resolves `perform` via a Rust [`JobRegistry`](https://docs.rs/chainmq/latest/chainmq/struct.JobRegistry.html); BullMQ uses string job names and dynamic handlers.
- **Observability:** BullMQ leans on events; ChainMQ adds tracing, optional Redis job logs, and the bundled web UI.

## Using `JobContext::queue` safely

- **Fine:** Enqueue follow-up jobs, read stats, use the same Redis prefix as the worker.
- **Avoid:** Completing, failing, or deleting the **current** job by id from inside `perform` while the worker will still call [`complete_job`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html#method.complete_job) or [`fail_after_perform_error`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html#method.fail_after_perform_error) after `perform` returns — that races and produces undefined behaviour unless you design around it.

## Migrating wait keys (priority buckets)

Older deployments used a single list `{prefix}:queue:{name}:wait`. That key is still drained **last** when claiming (after priority buckets `wait:p*`, `waitl:p*`). New jobs are pushed only to bucket keys. After draining legacy traffic, only bucket keys remain.
