use redis::Client;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::{collections::HashMap, sync::Arc};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug)]
struct CustomerRecord {
    id: String,
    name: String,
    email: String,
    age: u32,
    country: String,
    signup_date: String,
    total_orders: u32,
    total_spent: f64,
}

// Generate sample customer data for testing
fn generate_sample_data(num_records: usize) -> Vec<CustomerRecord> {
    let countries = vec![
        "USA",
        "UK",
        "Canada",
        "Germany",
        "France",
        "Japan",
        "Australia",
    ];
    let names = vec![
        "John Smith",
        "Sarah Johnson",
        "Michael Brown",
        "Emily Davis",
        "David Wilson",
        "Lisa Anderson",
        "James Taylor",
        "Maria Garcia",
        "Robert Miller",
        "Jennifer Lee",
    ];

    (0..num_records)
        .map(|i| {
            let name = names[i % names.len()];
            CustomerRecord {
                id: Uuid::new_v4().to_string(),
                name: name.to_string(),
                email: format!("{}{}@example.com", name.to_lowercase().replace(" ", "."), i),
                age: 18 + (i % 60) as u32,
                country: countries[i % countries.len()].to_string(),
                signup_date: format!("2024-{:02}-{:02}", (i % 12) + 1, (i % 28) + 1),
                total_orders: (i % 50) as u32,
                total_spent: (i as f64 * 12.34) % 5000.0,
            }
        })
        .collect()
}

// Simple in-memory dataset store for testing
#[derive(Debug)]
pub struct DatasetStore {
    datasets: HashMap<String, Vec<CustomerRecord>>,
}

impl DatasetStore {
    pub fn new() -> Self {
        let mut store = DatasetStore {
            datasets: HashMap::new(),
        };

        // Create sample datasets
        let customer_data = generate_sample_data(50_000);
        store
            .datasets
            .insert("customer_data_2024".to_string(), customer_data);

        let fraud_data = generate_sample_data(5_000);
        store
            .datasets
            .insert("urgent_fraud_analysis".to_string(), fraud_data);

        store
    }

    pub fn get_dataset(&self, dataset_id: &str) -> Option<&Vec<CustomerRecord>> {
        self.datasets.get(dataset_id)
    }

    pub fn get_batch(
        &self,
        dataset_id: &str,
        batch_num: usize,
        batch_size: usize,
    ) -> Option<Vec<&CustomerRecord>> {
        let dataset = self.get_dataset(dataset_id)?;
        let start_idx = (batch_num - 1) * batch_size;
        let end_idx = std::cmp::min(start_idx + batch_size, dataset.len());

        if start_idx >= dataset.len() {
            return Some(Vec::new());
        }

        Some(dataset[start_idx..end_idx].iter().collect())
    }

    pub fn get_total_records(&self, dataset_id: &str) -> usize {
        self.get_dataset(dataset_id).map(|d| d.len()).unwrap_or(0)
    }

    // Save dataset to file for inspection
    pub fn save_to_file(&self, dataset_id: &str, filename: &str) -> Result<()> {
        if let Some(dataset) = self.get_dataset(dataset_id) {
            let json = serde_json::to_string_pretty(dataset)?;
            let mut file = File::create(filename).expect("Cannot create file.");
            file.write_all(json.as_bytes())
                .expect("Can not write file.");
            println!("Saved {} records to {}", dataset.len(), filename);
        }
        Ok(())
    }
}

// Updated job implementation that uses real data
use chainmq::{
    AppContext, Job, JobContext, JobOptions, JobRegistry, Priority, Queue, QueueOptions, Result,
    WorkerBuilder, async_trait,
};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RealDataProcessingJob {
    pub dataset_id: String,
    pub batch_size: usize,
}

#[async_trait]
impl Job for RealDataProcessingJob {
    async fn perform(&self, _ctx: &JobContext) -> Result<()> {
        // In a real app, you'd get this from your app context or dependency injection
        let dataset_store = DatasetStore::new();

        let total_records = dataset_store.get_total_records(&self.dataset_id);
        if total_records == 0 {
            return Err(chainmq::ChainMQError::Worker(format!(
                "Dataset '{}' not found",
                self.dataset_id
            )));
        }

        println!(
            "[worker] Processing dataset '{}' with {} records",
            self.dataset_id, total_records
        );

        let total_batches = (total_records + self.batch_size - 1) / self.batch_size;
        let mut processed_records = 0;

        for batch_num in 1..=total_batches {
            // if ctx.is_cancelled() {
            //     println!("[worker] Job cancelled at batch {}", batch_num);
            //     return Err(chainmq::ChainMQError::JobCancelled);
            // }

            // Get real data batch
            if let Some(batch_data) =
                dataset_store.get_batch(&self.dataset_id, batch_num, self.batch_size)
            {
                println!(
                    "[worker] Processing batch {}/{} ({} records)",
                    batch_num,
                    total_batches,
                    batch_data.len()
                );

                // Process each record in the batch
                for record in &batch_data {
                    self.process_customer_record(record).await?;
                }

                processed_records += batch_data.len();
                let progress = (processed_records as f64 / total_records as f64) * 100.0;
                println!(
                    "[worker] Progress: {:.1}% ({}/{})",
                    progress, processed_records, total_records
                );

                // Small delay to simulate processing time
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }

        println!(
            "[worker] Completed processing {} records",
            processed_records
        );
        Ok(())
    }

    fn name() -> &'static str {
        "RealDataProcessingJob"
    }

    fn queue_name() -> &'static str {
        "data_processing"
    }
}

impl RealDataProcessingJob {
    async fn process_customer_record(&self, record: &CustomerRecord) -> Result<()> {
        // Simulate actual data processing
        // In reality, you might:
        // - Validate the data
        // - Transform it
        // - Save to another system
        // - Generate analytics

        if record.age < 18 {
            println!(
                "[worker] Warning: Invalid age for customer {}: {}",
                record.id, record.age
            );
        }

        if record.total_spent > 10000.0 {
            println!(
                "[worker] High-value customer detected: {} (spent: ${:.2})",
                record.name, record.total_spent
            );
        }

        // Simulate processing time
        tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
        Ok(())
    }
}

#[derive(Clone, Default)]
struct AppState;

impl AppContext for AppState {
    fn clone_context(&self) -> Arc<dyn AppContext> {
        Arc::new(self.clone())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging to see library tracing output if any
    tracing_subscriber::fmt::try_init().ok();

    println!("🔧 Generating sample customer data...");

    let store = DatasetStore::new();

    // Show dataset info
    for (dataset_id, dataset) in &store.datasets {
        println!("📊 Dataset '{}': {} records", dataset_id, dataset.len());

        // Show first few records
        if let Some(sample) = dataset.first() {
            println!(
                "   Sample: {} ({}) - {} orders, ${:.2} spent",
                sample.name, sample.email, sample.total_orders, sample.total_spent
            );
        }
    }

    // Save to files for inspection
    store.save_to_file("customer_data_2024", "customer_data_2024.json")?;
    store.save_to_file("urgent_fraud_analysis", "fraud_analysis.json")?;

    let redis_url = "redis://localhost:6379";
    let concurrency = 5usize;
    println!(
        "[boot] Spawning worker → redis='{}' concurrency={} queue='{}'",
        redis_url,
        concurrency,
        RealDataProcessingJob::queue_name()
    );

    let client = Client::open(redis_url)?;

    // STEP 2
    println!("[enqueue] Preparing QueueOptions and connecting to Redis...");
    let options = QueueOptions {
        redis_instance: Some(client.clone()),
        ..Default::default()
    };
    let queue = Queue::new(options).await?;
    println!(
        "[enqueue] Connected to Redis and initialized queue '{}'.",
        RealDataProcessingJob::queue_name()
    );

    let job = RealDataProcessingJob {
        dataset_id: "customer_data_2024".to_string(),
        batch_size: 100,
    };

    println!("[enqueue] Enqueuing simple RealDataProcessingJob...");

    let opts = JobOptions {
        delay_secs: Some(10),
        priority: Priority::High,
        attempts: 5,
        ..Default::default()
    };
    println!(
        "[enqueue] Enqueuing delayed/high-priority RealDataProcessingJob (delay=60s, attempts=5)..."
    );
    let job_id2 = queue.enqueue_with_options(job, opts).await?;
    println!(
        "[enqueue] Enqueued delayed RealDataProcessingJob with id={} — done.",
        job_id2
    );

    // STEP 3
    let app_state = Arc::new(AppState::default());
    let mut registry = JobRegistry::new();
    registry.register::<RealDataProcessingJob>();

    let mut worker = WorkerBuilder::new_with_redis_instance(client, registry)
        .with_app_context(app_state)
        .with_concurrency(concurrency)
        .with_queue_name(RealDataProcessingJob::queue_name())
        .spawn()
        .await?;

    println!("[worker] Starting worker event loops...");
    worker.start().await?;

    Ok(())
}
