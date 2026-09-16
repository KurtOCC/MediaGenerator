//! The in-process job queue.
//!
//! `POST /api/generate` writes a row and hands the id to this queue; a small
//! pool of workers picks it up, drives the provider and writes the terminal
//! status back. The browser follows along over Server-Sent Events.
//!
//! The queue is a hint, not the source of truth. Everything that matters is in
//! the `jobs` table, which is why a reload, a navigation away or a restart does
//! not lose a generation: the page asks the database what is still running.
//!
//! Phase 5 replaces [`run_mock`] with a real provider call. Nothing else here
//! changes.

use std::{sync::Arc, time::Duration};

use mediagenerator_domain::{ErrorCode, JobStatus, i18n::nb};
use mediagenerator_storage::{Database, jobs};
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

/// How many jobs may wait in the queue before submissions are refused.
///
/// This is the global concurrency limit: past this point the provider is the
/// bottleneck and accepting more work would only grow the wait invisibly.
const QUEUE_CAPACITY: usize = 64;

/// How many jobs are driven at once.
const WORKER_COUNT: usize = 4;

/// How many status events are buffered per subscriber.
///
/// A job emits at most a handful of events, so a slow reader would have to be
/// extremely far behind to miss one; if it does, the SSE handler re-reads the
/// current status from the database.
const EVENT_BUFFER: usize = 256;

/// How long the mock pretends to work.
const MOCK_DURATION: Duration = Duration::from_millis(2500);

/// A status change, broadcast to every interested SSE connection.
#[derive(Debug, Clone)]
pub struct JobEvent {
    /// Which job changed.
    pub job_id: Uuid,
    /// What it changed to.
    pub status: JobStatus,
}

/// Handle to the queue and its event stream.
#[derive(Debug, Clone)]
pub struct Jobs {
    sender: mpsc::Sender<Uuid>,
    events: broadcast::Sender<JobEvent>,
}

/// The queue was full; the caller should ask the user to try again shortly.
#[derive(Debug, thiserror::Error)]
#[error("the job queue is full")]
pub struct QueueFull;

impl Jobs {
    /// Starts the worker pool and returns a handle to it.
    ///
    /// The workers run until the last [`Jobs`] handle is dropped, which closes
    /// the channel and ends each receive loop.
    pub fn spawn(db: Database) -> Self {
        let (sender, receiver) = mpsc::channel::<Uuid>(QUEUE_CAPACITY);
        let (events, _) = broadcast::channel::<JobEvent>(EVENT_BUFFER);

        // One receiver shared by every worker, so a free worker takes the next
        // id rather than each worker owning a private backlog.
        let receiver = Arc::new(tokio::sync::Mutex::new(receiver));

        for worker in 0..WORKER_COUNT {
            let db = db.clone();
            let receiver = Arc::clone(&receiver);
            let events = events.clone();

            tokio::spawn(async move {
                loop {
                    let job_id = {
                        let mut guard = receiver.lock().await;
                        guard.recv().await
                    };
                    let Some(job_id) = job_id else {
                        tracing::debug!(worker, "job queue closed, worker stopping");
                        break;
                    };
                    process(&db, &events, job_id).await;
                }
            });
        }

        tracing::info!(
            workers = WORKER_COUNT,
            capacity = QUEUE_CAPACITY,
            "job queue started"
        );

        Self { sender, events }
    }

    /// Hands a job id to the workers.
    ///
    /// # Errors
    ///
    /// Returns [`QueueFull`] when the queue is at capacity. The row stays
    /// `queued`, and the start-up sweep will fail it if the process restarts
    /// before it is picked up.
    pub fn enqueue(&self, job_id: Uuid) -> Result<(), QueueFull> {
        self.sender.try_send(job_id).map_err(|error| {
            tracing::warn!(%job_id, %error, "could not enqueue job");
            QueueFull
        })
    }

    /// Subscribes to status changes for every job.
    ///
    /// The SSE handler filters by id; broadcasting to one channel keeps the
    /// worker from having to know who is listening.
    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.events.subscribe()
    }
}

/// Drives one job from `queued` to a terminal status.
async fn process(db: &Database, events: &broadcast::Sender<JobEvent>, job_id: Uuid) {
    // Compare-and-set: if the row is no longer queued, another worker has it,
    // or it was cancelled.
    let job = match jobs::mark_running(db, job_id).await {
        Ok(job) => job,
        Err(error) => {
            tracing::warn!(%job_id, %error, "job was not queued any more, skipping");
            return;
        }
    };

    publish(events, job_id, JobStatus::Running);
    tracing::info!(%job_id, media_type = %job.media_type, "job started");

    let outcome = run_mock(&job).await;

    match jobs::finish(db, job_id, &outcome).await {
        Ok(_) => {
            tracing::info!(%job_id, status = outcome.as_str(), "job finished");
            publish(events, job_id, outcome);
        }
        Err(error) => {
            // The work is done but the row could not be updated. Say so
            // loudly: the start-up sweep is what will clean this up.
            tracing::error!(%job_id, %error, "could not write the terminal job status");
        }
    }
}

/// Stands in for a provider call until phase 5.
async fn run_mock(job: &mediagenerator_domain::Job) -> JobStatus {
    tokio::time::sleep(MOCK_DURATION).await;

    // A prompt the real content filter would reject is simulated here, so the
    // failure path has something exercising it before a provider exists.
    if job.prompt.to_lowercase().contains("simuler feil") {
        return JobStatus::Failed {
            code: ErrorCode::ContentFilter,
            message: nb::ERR_CONTENT_FILTER.to_owned(),
        };
    }

    JobStatus::Succeeded
}

/// Broadcasts a status change, ignoring the case where nobody is listening.
fn publish(events: &broadcast::Sender<JobEvent>, job_id: Uuid, status: JobStatus) {
    // `send` fails only when there are no subscribers, which is the normal
    // state when the user has closed the tab. It is not an error.
    let _ = events.send(JobEvent { job_id, status });
}

/// Fails every job left unfinished by a previous process.
///
/// Run once at start-up. A row stuck on `running` would otherwise leave the
/// user staring at "Genererer…" forever, with nothing ever coming to move it.
///
/// # Errors
///
/// Returns an error when the sweep could not be applied.
pub async fn fail_orphans(db: &Database) -> Result<(), mediagenerator_storage::StorageError> {
    let count =
        jobs::fail_orphans(db, ErrorCode::Internal.as_str(), nb::ERR_JOB_INTERRUPTED).await?;
    if count > 0 {
        tracing::warn!(count, "failed jobs left unfinished by a previous process");
    }
    Ok(())
}
