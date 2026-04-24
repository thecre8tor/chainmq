// src/context.rs
use crate::{ChainMQError, JobId, JobMetadata, Queue, Result};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::Span;

/// Application context containing shared resources
pub trait AppContext: AsAny + Send + Sync + 'static {
    /// Clone the context for use in another thread
    fn clone_context(&self) -> Arc<dyn AppContext>;
}

/// Context provided to job execution
pub struct JobContext {
    pub job_id: JobId,
    pub job_metadata: JobMetadata,
    pub app_context: Arc<dyn AppContext>,
    pub span: Span,
    queue: Arc<Queue>,
    cancel: CancellationToken,
    response: Arc<Mutex<Option<serde_json::Value>>>,
}

impl JobContext {
    pub fn new(
        job_id: JobId,
        job_metadata: JobMetadata,
        app_context: Arc<dyn AppContext>,
        queue: Arc<Queue>,
        cancel: CancellationToken,
    ) -> Self {
        let span = tracing::info_span!(
            "job_execution",
            job_id = %job_id,
            job_name = %job_metadata.name,
            queue = %job_metadata.queue_name,
        );

        Self {
            job_id,
            job_metadata,
            app_context,
            span,
            queue,
            cancel,
            response: Arc::new(Mutex::new(None)),
        }
    }

    /// Same Redis [`Queue`] instance the worker uses (BullMQ-style `job.queue`).
    pub fn queue(&self) -> &Arc<Queue> {
        &self.queue
    }

    /// Cooperative cancellation (similar to BullMQ’s `AbortSignal`). Poll with
    /// [`CancellationToken::is_cancelled`] or `.cancelled().await`.
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Persist progress on this job’s metadata (visible in the dashboard API).
    pub async fn set_progress(&self, value: serde_json::Value) -> Result<()> {
        let mut meta = self
            .queue
            .get_job(&self.job_id)
            .await?
            .ok_or_else(|| ChainMQError::JobNotFound(self.job_id.clone()))?;
        meta.progress = Some(value);
        self.queue.save_job_metadata(&self.job_id, &meta).await
    }

    /// Attach a JSON-serializable result to this run. It is persisted on the job when it completes
    /// successfully (overwrites any previous value set during this execution).
    pub fn set_response(&self, value: serde_json::Value) {
        if let Ok(mut slot) = self.response.lock() {
            *slot = Some(value);
        }
    }

    pub(crate) fn take_response(&self) -> Option<serde_json::Value> {
        self.response.lock().ok().and_then(|mut g| g.take())
    }

    /// Get typed app context
    pub fn app<T: AppContext>(&self) -> Option<&T> {
        self.app_context.as_ref().as_any().downcast_ref::<T>()
    }
}

// Helper trait for downcasting
pub trait AsAny {
    fn as_any(&self) -> &dyn std::any::Any;
}

impl<T: AppContext> AsAny for T {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
