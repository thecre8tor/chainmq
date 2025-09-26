// src/worker.rs
use crate::{
    AppContext, ChainMQError, JobContext, JobId, JobRegistry, Queue, QueueOptions, Result,
};
use redis::Client;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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

    pub fn with_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.config.shutdown_timeout = timeout;
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
    shutdown_tx: broadcast::Sender<()>,
    is_shutting_down: Arc<AtomicBool>,
    active_jobs: Arc<Semaphore>,
}

impl Worker {
    async fn new(
        config: WorkerConfig,
        registry: JobRegistry,
        app_context: Arc<dyn AppContext>,
    ) -> Result<Self> {
        let queue = Arc::new(Queue::new(config.queue_options.clone()).await?);
        let semaphore = Arc::new(Semaphore::new(config.concurrency));
        let (shutdown_tx, _) = broadcast::channel(1);
        let is_shutting_down = Arc::new(AtomicBool::new(false));
        let active_jobs = Arc::new(Semaphore::new(0)); // Start at 0, increment for each job

        Ok(Self {
            config,
            queue,
            registry: Arc::new(registry),
            app_context,
            semaphore,
            handles: Vec::new(),
            shutdown_tx,
            is_shutting_down,
            active_jobs,
        })
    }

    /// Start the worker
    pub async fn start(&mut self) -> Result<()> {
        info!(
            "Starting worker {} with concurrency {}",
            self.config.worker_id, self.config.concurrency
        );

        // 🎯 Setup signal handling for graceful shutdown
        self.setup_signal_handlers();

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

        // 🎯 Wait for shutdown signal
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        shutdown_rx.recv().await.ok();

        // 🛑 Graceful shutdown initiated
        self.graceful_shutdown().await;

        Ok(())
    }

    fn setup_signal_handlers(&self) {
        let shutdown_tx = self.shutdown_tx.clone();
        let worker_id = self.config.worker_id.clone();

        tokio::spawn(async move {
            Self::wait_for_shutdown_signal().await;
            info!("Shutdown signal received by worker {}", worker_id);
            let _ = shutdown_tx.send(());
        });
    }

    async fn wait_for_shutdown_signal() {
        use tokio::signal;

        #[cfg(unix)]
        {
            use signal::unix::{SignalKind, signal};

            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to setup SIGTERM handler");
            let mut sigint =
                signal(SignalKind::interrupt()).expect("Failed to setup SIGINT handler");

            tokio::select! {
                _ = sigterm.recv() => info!("SIGTERM received"),
                _ = sigint.recv() => info!("SIGINT received"),
                _ = signal::ctrl_c() => info!("CTRL+C received"),
            }
        }

        #[cfg(not(unix))]
        {
            signal::ctrl_c()
                .await
                .expect("Failed to setup CTRL+C handler");
            info!("CTRL+C received");
        }
    }

    /// Perform graceful shutdown
    async fn graceful_shutdown(&mut self) {
        info!(
            "Initiating graceful shutdown for worker {}",
            self.config.worker_id
        );

        // 🛑 Step 1: Set shutdown flag (stop accepting new jobs)
        self.is_shutting_down.store(true, Ordering::SeqCst);
        info!("Worker stopped accepting new jobs");

        // 🛑 Step 2: Cancel background tasks (polling, delayed processing, etc.)
        for handle in self.handles.drain(..) {
            handle.abort();
        }
        info!("Background tasks cancelled");

        // 🛑 Step 3: Wait for active jobs to complete (with timeout)
        let active_job_count = self.config.concurrency - self.semaphore.available_permits();
        if active_job_count > 0 {
            info!(
                "Waiting for {} active jobs to complete...",
                active_job_count
            );

            match timeout(
                self.config.shutdown_timeout,
                self.wait_for_jobs_completion(),
            )
            .await
            {
                Ok(_) => {
                    info!("All jobs completed successfully during shutdown");
                }
                Err(_) => {
                    let remaining = self.config.concurrency - self.semaphore.available_permits();
                    warn!(
                        "Shutdown timeout reached. {} jobs may still be running",
                        remaining
                    );
                }
            }

            info!("Worker {} shutdown complete", self.config.worker_id);
        }
    }

    /// Wait for all active jobs to complete
    async fn wait_for_jobs_completion(&self) {
        // Wait until all semaphore permits are available (no jobs running)
        let permits = match self
            .semaphore
            .clone()
            .acquire_many_owned(self.config.concurrency as u32)
            .await
        {
            Ok(permits) => permits,
            Err(_) => return, // Semaphore closed
        };

        // Release permits immediately - we just wanted to wait for availability
        drop(permits);
    }

    /// Stop the worker gracefully (public API)
    pub async fn stop(&mut self) {
        info!("Stop requested for worker {}", self.config.worker_id);
        let _ = self.shutdown_tx.send(());
    }

    /// Force immediate shutdown (emergency only)
    pub async fn force_stop(&mut self) {
        self.is_shutting_down.store(true, Ordering::SeqCst);
        for handle in self.handles.drain(..) {
            handle.abort();
        }
    }

    async fn spawn_worker_loop(&self) -> JoinHandle<()> {
        let queue = Arc::clone(&self.queue);
        let registry = Arc::clone(&self.registry);
        let app_context = Arc::clone(&self.app_context);
        let semaphore = Arc::clone(&self.semaphore);
        let queue_name = self.config.queue_options.name.clone();
        let worker_id = self.config.worker_id.clone();
        let poll_interval = self.config.poll_interval;
        let is_shutting_down = Arc::clone(&self.is_shutting_down);

        tokio::spawn(async move {
            let mut interval = interval(poll_interval);

            loop {
                // 🛑 Check if we're shutting down
                if is_shutting_down.load(Ordering::SeqCst) {
                    info!("Worker loop stopping - shutdown initiated");
                    break;
                }

                interval.tick().await;

                // Try to claim a job
                match queue.claim_job(&queue_name, &worker_id).await {
                    Ok(Some(job_id)) => {
                        // 🛑 Double-check shutdown status before acquiring permit
                        if is_shutting_down.load(Ordering::SeqCst) {
                            // Return job to queue if we're shutting down
                            // TODO: Add complete stalled job detection and recovery system
                            // let _ = queue.release_job(&job_id, &queue_name).await;
                            break;
                        }

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
                        let job_shutdown_flag = Arc::clone(&is_shutting_down);

                        tokio::spawn(async move {
                            let _permit = permit; // Keep permit alive

                            if let Err(e) = Self::execute_job(
                                job_queue,
                                job_registry,
                                job_app_context,
                                job_id,
                                job_queue_name,
                                job_shutdown_flag,
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

            info!("Worker loop terminated");
        })
    }

    async fn spawn_delayed_processor(&self) -> JoinHandle<()> {
        let queue = Arc::clone(&self.queue);
        let queue_name = self.config.queue_options.name.clone();
        let is_shutting_down = Arc::clone(&self.is_shutting_down);

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(5));

            loop {
                // 🛑 Check shutdown status
                if is_shutting_down.load(Ordering::SeqCst) {
                    info!("Delayed processor stopping");
                    break;
                }

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
        let is_shutting_down = Arc::clone(&self.is_shutting_down);

        tokio::spawn(async move {
            // TODO: Implement stalled job detection and recovery
            // This would check for jobs that have been active too long
            // and either retry them or mark them as failed
            loop {
                // 🛑 Check shutdown status
                if is_shutting_down.load(Ordering::SeqCst) {
                    info!("Stalled checker stopping");
                    break;
                }

                // TODO: Implement stalled job detection and recovery
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
        is_shutting_down: Arc<AtomicBool>,
    ) -> Result<()> {
        let start_time = std::time::Instant::now();

        // 🛑 Check if shutdown was initiated before job execution
        if is_shutting_down.load(Ordering::SeqCst) {
            warn!("Job {} skipped due to shutdown", job_id);
            return Ok(());
        }

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
            is_shutting_down: self.is_shutting_down.load(Ordering::SeqCst),
            active_jobs: self.config.concurrency - self.semaphore.available_permits(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkerStats {
    pub worker_id: String,
    pub concurrency: usize,
    pub available_permits: usize,
    pub queue_name: String,
    pub is_shutting_down: bool,
    pub active_jobs: usize,
}
