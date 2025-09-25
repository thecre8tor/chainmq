// src/worker.rs
use crate::{
    AppContext, ChainMQError, JobContext, JobId, JobRegistry, Queue, QueueOptions, Result,
};
use redis::Client;
use std::sync::Arc;
use tokio::{
    sync::{Semaphore, broadcast},
    task::JoinHandle,
    time::{Duration, interval, timeout},
};
use tracing::{error, info, instrument, warn};

/// Worker configuration
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub queue_options: QueueOptions,
    pub concurrency: usize,
    pub poll_interval: Duration,
    pub stalled_job_check_interval: Duration,
    pub worker_id: String,
    pub shutdown_timeout: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            queue_options: QueueOptions::default(),
            concurrency: 10,
            poll_interval: Duration::from_millis(100),
            stalled_job_check_interval: Duration::from_secs(30),
            worker_id: format!("worker-{}", uuid::Uuid::new_v4()),
            shutdown_timeout: Duration::from_secs(30),
        }
    }
}

/// Worker builder for fluent configuration
pub struct WorkerBuilder {
    config: WorkerConfig,
    registry: JobRegistry,
    app_context: Option<Arc<dyn AppContext>>,
}

impl WorkerBuilder {
    pub fn new_with_redis_uri(redis_url: impl Into<String>, registry: JobRegistry) -> Self {
        let mut config = WorkerConfig::default();
        config.queue_options.redis_url = redis_url.into();

        Self {
            config,
            registry,
            app_context: None,
        }
    }

    pub fn new_with_redis_instance(redis_client: Client, registry: JobRegistry) -> Self {
        let mut config = WorkerConfig::default();
        config.queue_options.redis_instance = Some(redis_client);

        Self {
            config,
            registry,
            app_context: None,
        }
    }

    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.config.concurrency = concurrency;
        self
    }

    pub fn with_queue_name(mut self, name: impl Into<String>) -> Self {
        self.config.queue_options.name = name.into();
        self
    }

    pub fn with_app_context(mut self, ctx: Arc<dyn AppContext>) -> Self {
        self.app_context = Some(ctx);
        self
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.config.poll_interval = interval;
        self
    }

    pub async fn spawn(self) -> Result<Worker> {
        let app_context = self
            .app_context
            .ok_or_else(|| ChainMQError::Worker("App context is required".to_string()))?;

        Worker::new(self.config, self.registry, app_context).await
    }
}

/// Job worker that processes queued jobs
pub struct Worker {
    config: WorkerConfig,
    queue: Arc<Queue>,
    registry: Arc<JobRegistry>,
    app_context: Arc<dyn AppContext>,
    semaphore: Arc<Semaphore>,
    handles: Vec<JoinHandle<()>>,
}

impl Worker {
    async fn new(
        config: WorkerConfig,
        registry: JobRegistry,
        app_context: Arc<dyn AppContext>,
    ) -> Result<Self> {
        let queue = Arc::new(Queue::new(config.queue_options.clone()).await?);
        let semaphore = Arc::new(Semaphore::new(config.concurrency));

        Ok(Self {
            config,
            queue,
            registry: Arc::new(registry),
            app_context,
            semaphore,
            handles: Vec::new(),
        })
    }

    /// Start the worker
    pub async fn start(&mut self) -> Result<()> {
        info!(
            "Starting worker {} with concurrency {}",
            self.config.worker_id, self.config.concurrency
        );

        // Start main worker loop
        let worker_handle = self.spawn_worker_loop().await;
        self.handles.push(worker_handle);

        // Start delayed job processor
        let delayed_handle = self.spawn_delayed_processor().await;
        self.handles.push(delayed_handle);

        // Start stalled job checker
        let stalled_handle = self.spawn_stalled_checker().await;
        self.handles.push(stalled_handle);

        info!("Worker started successfully");
        Ok(())
    }

    /// Stop the worker gracefully
    pub async fn stop(&mut self) {
        info!("Stopping worker {}", self.config.worker_id);

        // Cancel all background tasks
        for handle in self.handles.drain(..) {
            handle.abort();
        }

        info!("Worker stopped");
    }

    async fn spawn_worker_loop(&self) -> JoinHandle<()> {
        let queue = Arc::clone(&self.queue);
        let registry = Arc::clone(&self.registry);
        let app_context = Arc::clone(&self.app_context);
        let semaphore = Arc::clone(&self.semaphore);
        let queue_name = self.config.queue_options.name.clone();
        let worker_id = self.config.worker_id.clone();
        let poll_interval = self.config.poll_interval;

        tokio::spawn(async move {
            let mut interval = interval(poll_interval);

            loop {
                interval.tick().await;

                // Try to claim a job
                match queue.claim_job(&queue_name, &worker_id).await {
                    Ok(Some(job_id)) => {
                        // Acquire semaphore permit for concurrency control
                        let permit = match semaphore.clone().acquire_owned().await {
                            Ok(permit) => permit,
                            Err(_) => {
                                error!("Failed to acquire semaphore permit");
                                continue;
                            }
                        };

                        // Spawn job execution task
                        let job_queue = Arc::clone(&queue);
                        let job_registry = Arc::clone(&registry);
                        let job_app_context = Arc::clone(&app_context);
                        let job_queue_name = queue_name.clone();

                        tokio::spawn(async move {
                            let _permit = permit; // Keep permit alive

                            if let Err(e) = Self::execute_job(
                                job_queue,
                                job_registry,
                                job_app_context,
                                job_id,
                                job_queue_name,
                            )
                            .await
                            {
                                error!("Job execution failed: {}", e);
                            }
                        });
                    }
                    Ok(None) => {
                        // No jobs available, continue polling
                    }
                    Err(e) => {
                        error!("Failed to claim job: {}", e);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        })
    }

    async fn spawn_delayed_processor(&self) -> JoinHandle<()> {
        let queue = Arc::clone(&self.queue);
        let queue_name = self.config.queue_options.name.clone();

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(5));

            loop {
                interval.tick().await;

                match queue.process_delayed(&queue_name).await {
                    Ok(moved_count) => {
                        if moved_count > 0 {
                            info!("Moved {} delayed jobs to waiting", moved_count);
                        }
                    }
                    Err(e) => {
                        error!("Failed to process delayed jobs: {}", e);
                    }
                }
            }
        })
    }

    async fn spawn_stalled_checker(&self) -> JoinHandle<()> {
        let _queue = Arc::clone(&self.queue);
        let _interval = self.config.stalled_job_check_interval;

        tokio::spawn(async move {
            // TODO: Implement stalled job detection and recovery
            // This would check for jobs that have been active too long
            // and either retry them or mark them as failed
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        })
    }

    #[instrument(skip_all, fields(job_id = %job_id))]
    async fn execute_job(
        queue: Arc<Queue>,
        registry: Arc<JobRegistry>,
        app_context: Arc<dyn AppContext>,
        job_id: JobId,
        queue_name: String,
    ) -> Result<()> {
        let start_time = std::time::Instant::now();

        // Get job metadata
        let metadata = match queue.get_job(&job_id).await? {
            Some(metadata) => metadata,
            None => {
                error!("Job {} not found", job_id);
                return Err(ChainMQError::JobNotFound(job_id));
            }
        };

        // Create job context
        let job_context = JobContext::new(job_id.clone(), metadata.clone(), app_context);

        // Execute the job
        let result = registry
            .execute_job(&metadata.name, metadata.payload.clone(), &job_context)
            .await;

        let execution_time = start_time.elapsed();

        // Handle result
        match result {
            Ok(()) => {
                queue.complete_job(&job_id, &queue_name).await?;
                info!(
                    "Job {} completed successfully in {:?}",
                    job_id, execution_time
                );

                // Record metrics using global static
                // crate::metrics::JOB_DURATION
                //     .with_label_values(&[&queue_name, &metadata.name, "success"])
                //     .observe(execution_time.as_secs_f64());
            }
            Err(e) => {
                let error_msg = format!("{}", e);
                queue
                    .fail_job(&job_id, &queue_name, &error_msg, &metadata)
                    .await?;
                error!("Job {} failed: {}", job_id, error_msg);

                // Record metrics using global static
                // crate::metrics::JOB_DURATION
                //     .with_label_values(&[&queue_name, &metadata.name, "failure"])
                //     .observe(execution_time.as_secs_f64());
            }
        }

        Ok(())
    }

    /// Get worker statistics
    pub async fn get_stats(&self) -> WorkerStats {
        WorkerStats {
            worker_id: self.config.worker_id.clone(),
            concurrency: self.config.concurrency,
            available_permits: self.semaphore.available_permits(),
            queue_name: self.config.queue_options.name.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkerStats {
    pub worker_id: String,
    pub concurrency: usize,
    pub available_permits: usize,
    pub queue_name: String,
}
