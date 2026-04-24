//! `tracing` layer that appends events under the `job_execution` span to Redis
//! (see [`crate::Queue::append_job_log_line`]).
//!
//! When [`crate::worker::WorkerConfig::tracing_job_logs`] is enabled, the worker may install this
//! automatically (with `fmt` and `EnvFilter`) if no global subscriber is set yet. For a custom
//! subscriber, register [`job_logs_layer`] yourself and use [`crate::WorkerBuilder::with_shared_queue`]
//! with the same [`Arc<Queue>`].

use crate::{JobId, JobLogLine, Queue};
use chrono::SecondsFormat;
use std::fmt;
use std::sync::Arc;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

#[derive(Clone)]
struct StoredJobId(JobId);

#[derive(Default)]
struct JobIdFieldVisitor {
    raw: Option<String>,
}

impl Visit for JobIdFieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "job_id" {
            self.raw = Some(value.to_string());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "job_id" {
            self.raw = Some(format!("{value:?}"));
        }
    }
}

fn parse_job_id_field(raw: &str) -> Option<JobId> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix("JobId(").and_then(|r| r.strip_suffix(')')) {
        let inner = rest.trim();
        if inner.starts_with('"') {
            return serde_json::from_str::<String>(inner)
                .ok()
                .filter(|x| !x.is_empty())
                .map(JobId);
        }
        return (!inner.is_empty()).then(|| JobId(inner.to_string()));
    }
    let plain = s.trim_matches('"');
    (!plain.is_empty()).then(|| JobId(plain.to_string()))
}

#[derive(Default)]
struct EventMessageVisitor {
    buf: String,
}

impl Visit for EventMessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.push_field(field.name(), value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        use std::fmt::Write;
        if self.buf.is_empty() && field.name() == "message" {
            let _ = write!(&mut self.buf, "{value:?}");
            return;
        }
        if !self.buf.is_empty() {
            self.buf.push(' ');
        }
        let _ = write!(&mut self.buf, "{}={:?}", field.name(), value);
    }
}

impl EventMessageVisitor {
    fn push_field(&mut self, name: &str, value: &str) {
        if !self.buf.is_empty() {
            self.buf.push(' ');
        }
        self.buf.push_str(name);
        self.buf.push('=');
        self.buf.push_str(value);
    }
}

/// A [`tracing_subscriber::Layer`] that records tracing events emitted while a job runs
/// (under the `job_execution` span from [`crate::JobContext`]) into Redis via [`Queue`].
///
/// Requires a Tokio runtime (`try_current`); if none is installed, events are ignored.
///
/// Events whose target starts with `chainmq::` are skipped to avoid worker noise in the UI.
#[derive(Clone)]
pub struct JobLogLayer {
    queue: Arc<Queue>,
}

impl JobLogLayer {
    pub fn new(queue: Arc<Queue>) -> Self {
        Self { queue }
    }
}

/// Build a [`JobLogLayer`] for use with `tracing_subscriber::registry::Registry` (e.g. `with`).
pub fn job_logs_layer(queue: Arc<Queue>) -> JobLogLayer {
    JobLogLayer::new(queue)
}

/// If no global tracing subscriber is installed, sets one: `EnvFilter` (`RUST_LOG` or `info`),
/// stdout `fmt`, and [`job_logs_layer`] for this queue. Called from [`crate::Worker`] startup when
/// [`crate::worker::WorkerConfig::tracing_job_logs`] is `true`.
pub(crate) fn install_default_subscriber_with_job_logs_if_unset(queue: Arc<Queue>) {
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::registry::Registry::default()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(job_logs_layer(queue))
        .try_init();
}

impl<S> Layer<S> for JobLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn register_callsite(
        &self,
        metadata: &'static Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        if metadata.is_span() && metadata.name() == "job_execution" {
            tracing::subscriber::Interest::always()
        } else {
            tracing::subscriber::Interest::sometimes()
        }
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.metadata().name() != "job_execution" {
            return;
        }
        let mut vis = JobIdFieldVisitor::default();
        attrs.record(&mut vis);
        let Some(raw) = vis.raw else { return };
        let Some(job_id) = parse_job_id_field(&raw) else {
            return;
        };
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(StoredJobId(job_id));
        }
    }

    fn on_record(&self, _span: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.lookup_current() else {
            return;
        };
        if span.metadata().name() != "job_execution" {
            return;
        }
        if span.extensions().get::<StoredJobId>().is_some() {
            return;
        }
        let mut vis = JobIdFieldVisitor::default();
        values.record(&mut vis);
        let Some(raw) = vis.raw else { return };
        let Some(job_id) = parse_job_id_field(&raw) else {
            return;
        };
        span.extensions_mut().insert(StoredJobId(job_id));
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let target = event.metadata().target();
        if target.starts_with("chainmq::") {
            return;
        }

        let mut job_id: Option<JobId> = None;
        if let Some(span) = ctx.event_span(event) {
            for s in span.scope().from_root() {
                if let Some(st) = s.extensions().get::<StoredJobId>() {
                    job_id = Some(st.0.clone());
                    break;
                }
            }
        }
        if job_id.is_none()
            && let Some(cur) = ctx.lookup_current()
        {
            for s in cur.scope().from_root() {
                if let Some(st) = s.extensions().get::<StoredJobId>() {
                    job_id = Some(st.0.clone());
                    break;
                }
            }
        }
        let Some(job_id) = job_id else {
            return;
        };

        let mut msg_vis = EventMessageVisitor::default();
        event.record(&mut msg_vis);
        let message = if msg_vis.buf.is_empty() {
            event.metadata().name().to_string()
        } else {
            msg_vis.buf
        };

        let ts = chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let level = event.metadata().level().as_str().to_string();
        let line = JobLogLine { ts, level, message };

        let queue = self.queue.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = queue.append_job_log_line(&job_id, line).await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_job_id_field_accepts_uuid_string() {
        let u = uuid::Uuid::new_v4();
        let id = parse_job_id_field(&u.to_string()).expect("uuid");
        assert_eq!(id.0, u.to_string());
    }

    #[test]
    fn parse_job_id_field_accepts_arbitrary_string() {
        let id = parse_job_id_field("invoice-42/line-3").expect("custom");
        assert_eq!(id.0, "invoice-42/line-3");
    }

    #[test]
    fn parse_job_id_field_accepts_debug_tuple() {
        let id = JobId("my-custom-id".into());
        let s = format!("{id:?}");
        let parsed = parse_job_id_field(&s).expect("debug");
        assert_eq!(parsed, id);
    }
}
