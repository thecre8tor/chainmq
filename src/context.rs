// src/context.rs
use crate::{JobId, JobMetadata};
use std::sync::Arc;
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
}

impl JobContext {
    pub fn new(job_id: JobId, job_metadata: JobMetadata, app_context: Arc<dyn AppContext>) -> Self {
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
        }
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
