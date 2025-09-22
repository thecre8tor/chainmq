// src/registry.rs
use crate::{Job, JobContext, Result, RsBullError};
use std::any::TypeId;
use std::collections::HashMap;

/// Registry for job types, enabling deserialization by name
pub struct JobRegistry {
    jobs: HashMap<String, Box<dyn JobExecutor>>,
    type_names: HashMap<TypeId, String>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
            type_names: HashMap::new(),
        }
    }

    /// Register a job type
    pub fn register<T: Job>(&mut self) -> &mut Self {
        let name = T::name().to_string();
        let type_id = TypeId::of::<T>();

        self.jobs
            .insert(name.clone(), Box::new(TypedJobExecutor::<T>::new()));
        self.type_names.insert(type_id, name);
        self
    }

    /// Execute a job by name with payload
    pub async fn execute_job(
        &self,
        name: &str,
        payload: serde_json::Value,
        ctx: &JobContext,
    ) -> Result<()> {
        let executor = self
            .jobs
            .get(name)
            .ok_or_else(|| RsBullError::Registry(format!("Job type '{}' not registered", name)))?;

        executor.execute(payload, ctx).await
    }

    /// Get registered job names
    pub fn job_names(&self) -> Vec<String> {
        self.jobs.keys().cloned().collect()
    }

    /// Check if job type is registered
    pub fn contains_job(&self, name: &str) -> bool {
        self.jobs.contains_key(name)
    }
}

#[async_trait::async_trait]
trait JobExecutor: Send + Sync {
    async fn execute(&self, payload: serde_json::Value, ctx: &JobContext) -> Result<()>;
}

struct TypedJobExecutor<T: Job> {
    _phantom: std::marker::PhantomData<T>,
}

impl<T: Job> TypedJobExecutor<T> {
    fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait::async_trait]
impl<T: Job> JobExecutor for TypedJobExecutor<T> {
    async fn execute(&self, payload: serde_json::Value, ctx: &JobContext) -> Result<()> {
        let job: T = serde_json::from_value(payload)?;
        job.perform(ctx).await
    }
}
