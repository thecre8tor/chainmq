# Repeatable / cron jobs (design sketch)

BullMQ supports first-class repeatable jobs (cron-like schedules). ChainMQ does **not** implement this yet. This document outlines a possible direction if the feature is added later.

## Goals

- Schedule work at fixed intervals or cron expressions without duplicate concurrent runs for the same logical schedule key.
- Interact cleanly with existing **delayed** jobs (`JobOptions::delay_secs`), **custom `job_id`**, and **retry** semantics.

## Building blocks

1. **Scheduler index:** A Redis sorted set keyed by `{prefix}:repeat:{queue_name}` (or global) with members `schedule_id` and scores = next run time (unix seconds), updated after each fire.
2. **Dedupe key:** Hash or string key so the same schedule does not enqueue two physical jobs for the same tick when multiple workers poll.
3. **Promotion loop:** Similar to [`Queue::process_delayed`](https://docs.rs/chainmq/latest/chainmq/struct.Queue.html#method.process_delayed) — a periodic task moves due repeat entries into the normal **wait** buckets via existing `enqueue_with_options` (or an internal path that reuses metadata layout).
4. **Job identity:** Either synthetic stable `job_id` per schedule tick, or BullMQ-style `repeat` options stored in `JobOptions` / metadata extensions.

## Open questions

- Whether repeat definitions live in **code** (Rust `Job` impl) only or are also **Redis-configurable** for ops.
- Back-pressure when a tick is missed (catch-up one job vs many).
- Interaction with **pause** / dashboard controls.

No implementation in the crate follows this document until a dedicated epic is scoped.
