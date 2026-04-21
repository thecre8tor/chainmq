# ChainMQ Web UI

A beautiful, modern web interface for managing and monitoring your ChainMQ job queues.

## Features

- 📊 **Real-time Queue Statistics**: View waiting, active, delayed, and failed job counts
- 📋 **Job Management**: Browse jobs by state (waiting, active, delayed, failed)
- 🔍 **Job Details**: View complete job metadata and payload
- 🔄 **Retry Failed Jobs**: Retry failed jobs with a single click
- 🗑️ **Delete Jobs**: Remove jobs from the queue
- 🔄 **Auto-refresh**: Automatically updates every 5 seconds

## Running the Web UI

1. Make sure Redis is running:
   ```bash
   redis-server
   ```

2. Start the web UI server:
   ```bash
   cargo run --example web_ui
   ```

3. Open your browser and navigate to:
   ```
   http://127.0.0.1:8080
   ```

## Configuration

You can configure the Redis connection using the `REDIS_URL` environment variable:

```bash
REDIS_URL=redis://127.0.0.1:6379 cargo run --example web_ui
```

## HTTP data loading

The SPA talks to `{base}/api/...` on the same origin. Those routes are **internal** to the dashboard (browser same-origin fetches only), not a documented public REST API for scripts or integrations.

## Usage

1. **Select a Queue**: Use the dropdown in the header to select a queue to monitor
2. **View Statistics**: The dashboard shows real-time counts for each job state
3. **Browse Jobs**: Click on the tabs (Waiting, Active, Delayed, Failed) to view jobs in each state
4. **View Job Details**: Click on any job card to see full job metadata
5. **Retry Jobs**: For failed jobs, click "Retry Job" to requeue them
6. **Delete Jobs**: Click "Delete Job" to permanently remove a job from the queue

## Screenshots

The UI features a modern, gradient-based design with:
- Color-coded job states
- Responsive layout for mobile and desktop
- Real-time updates
- Intuitive navigation
