//! The in-process job queue.
//!
//! `POST /api/generate` writes a row and hands the id to this queue; a small
//! pool of workers picks it up, calls the provider, stores the result in the
//! private blob container and writes the terminal status back. The browser
//! follows along over Server-Sent Events.
//!
//! The queue is a hint, not the source of truth. Everything that matters is in
//! the `jobs` table, which is why a reload, a navigation away or a restart does
//! not lose a generation: the page asks the database what is still running.

use std::{sync::Arc, time::Duration};

use mediagenerator_domain::{ErrorCode, Job, JobStatus, MediaType, NewAsset, i18n::nb};
use mediagenerator_providers::{
    GeneratedMedia, GenerationRequest, JobStatus as ProviderStatus, Providers, video::POLL_INTERVAL,
};
use mediagenerator_storage::{BlobStore, Database, assets, jobs};
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
const EVENT_BUFFER: usize = 256;

/// Longest an asynchronous generation may take before it is given up on.
///
/// Sora is the only provider that gets anywhere near this. A job that has run
/// this long is not going to finish usefully, and holding a worker on it
/// starves everyone else.
const MAX_ASYNC_WAIT: Duration = Duration::from_secs(15 * 60);

/// A status change, broadcast to every interested SSE connection.
#[derive(Debug, Clone)]
pub struct JobEvent {
    /// Which job changed.
    pub job_id: Uuid,
    /// What it changed to.
    pub status: JobStatus,
}

/// Everything a worker needs to run a job.
#[derive(Clone)]
pub struct Runtime {
    /// Connection pool.
    pub db: Database,
    /// The three Azure AI Foundry clients.
    pub providers: Arc<Providers>,
    /// The private container generated media is stored in.
    pub blobs: Arc<BlobStore>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Runtime")
    }
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
    pub fn spawn(runtime: Runtime) -> Self {
        let (sender, receiver) = mpsc::channel::<Uuid>(QUEUE_CAPACITY);
        let (events, _) = broadcast::channel::<JobEvent>(EVENT_BUFFER);

        // One receiver shared by every worker, so a free worker takes the next
        // id rather than each worker owning a private backlog.
        let receiver = Arc::new(tokio::sync::Mutex::new(receiver));

        for worker in 0..WORKER_COUNT {
            let runtime = runtime.clone();
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
                    process(&runtime, &events, job_id).await;
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
    /// Returns [`QueueFull`] when the queue is at capacity.
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
async fn process(runtime: &Runtime, events: &broadcast::Sender<JobEvent>, job_id: Uuid) {
    // Compare-and-set: if the row is no longer queued, another worker has it,
    // or it was cancelled.
    let job = match jobs::mark_running(&runtime.db, job_id).await {
        Ok(job) => job,
        Err(error) => {
            tracing::warn!(%job_id, %error, "job was not queued any more, skipping");
            return;
        }
    };

    publish(events, job_id, JobStatus::Running);
    tracing::info!(%job_id, media_type = %job.media_type, "job started");

    let outcome = match generate(runtime, &job).await {
        Ok(media) => match store(runtime, &job, media).await {
            Ok(()) => JobStatus::Succeeded,
            Err(error) => {
                // The media exists but could not be kept. Failing is honest:
                // there is nothing to show the user.
                tracing::error!(%job_id, %error, "could not store the generated media");
                JobStatus::Failed {
                    code: ErrorCode::Internal,
                    message: nb::ERR_INTERNAL.to_owned(),
                }
            }
        },
        Err(status) => status,
    };

    match jobs::finish(&runtime.db, job_id, &outcome).await {
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

/// Runs the right provider for a job and returns the finished media.
///
/// The error side is a [`JobStatus::Failed`] rather than an error type, because
/// every failure here ends the job and the caller has nothing to add.
async fn generate(runtime: &Runtime, job: &Job) -> Result<GeneratedMedia, JobStatus> {
    let provider = runtime.providers.for_media(job.media_type);

    let request = GenerationRequest {
        media_type: job.media_type,
        prompt: job.prompt.clone(),
        parameters: job.parameters.clone(),
        correlation_id: job.id.to_string(),
    };

    let submitted = provider.submit(request).await.map_err(failed)?;

    // Image and speech are done here; only video has anything to poll.
    if let Some(media) = submitted.media {
        return Ok(media);
    }

    if let Some(provider_job_id) = submitted.id.as_deref()
        && let Err(error) = jobs::set_provider_job_id(&runtime.db, job.id, provider_job_id).await
    {
        // Losing the provider's id costs traceability, not the generation.
        tracing::warn!(job_id = %job.id, %error, "could not record the provider job id");
    }

    let deadline = tokio::time::Instant::now() + MAX_ASYNC_WAIT;

    loop {
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!(job_id = %job.id, "gave up waiting for the provider");
            return Err(JobStatus::Failed {
                code: ErrorCode::Upstream,
                message: nb::ERR_GENERATION_TIMEOUT.to_owned(),
            });
        }

        tokio::time::sleep(POLL_INTERVAL).await;

        match provider.poll(&submitted).await {
            Ok(ProviderStatus::Running) => continue,
            Ok(ProviderStatus::Succeeded(media)) => return Ok(*media),
            Ok(ProviderStatus::Failed(error)) | Err(error) => return Err(failed(error)),
        }
    }
}

/// Uploads the media and records the asset row.
async fn store(
    runtime: &Runtime,
    job: &Job,
    media: GeneratedMedia,
) -> Result<(), mediagenerator_storage::StorageError> {
    let path = blob_path(job.id, job.media_type, media.extension);
    let size = media.bytes.len() as i64;

    runtime
        .blobs
        .upload(&path, &media.content_type, media.bytes)
        .await?;

    assets::create(
        &runtime.db,
        &NewAsset {
            job_id: job.id,
            blob_path: path,
            content_type: media.content_type,
            size_bytes: size,
            duration_ms: media.duration_ms,
            width: media.width,
            height: media.height,
        },
    )
    .await?;

    Ok(())
}

/// Builds the path a job's media is stored under.
///
/// Dated prefixes keep the container browsable and make a lifecycle rule
/// straightforward; the job id makes the name unique without a lookup.
fn blob_path(job_id: Uuid, media_type: MediaType, extension: &str) -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{}/{:02}/{}/{job_id}.{extension}",
        now.year(),
        u8::from(now.month()),
        media_type.as_str()
    )
}

/// Turns a provider error into the terminal status stored on the job.
fn failed(error: mediagenerator_providers::ProviderError) -> JobStatus {
    tracing::warn!(%error, "generation failed");
    JobStatus::Failed {
        code: error.code(),
        message: error.user_message().to_owned(),
    }
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
/// user staring at "Genererer …" forever, with nothing coming to move it.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blob_path_is_dated_typed_and_unique() {
        let id = Uuid::new_v4();
        let path = blob_path(id, MediaType::Image, "png");

        let parts: Vec<_> = path.split('/').collect();
        assert_eq!(parts.len(), 4, "expected year/month/type/file, got {path}");
        assert_eq!(parts[1].len(), 2, "the month should be zero-padded");
        assert_eq!(parts[2], "image");
        assert_eq!(parts[3], format!("{id}.png"));
    }

    #[test]
    fn each_media_type_gets_its_own_prefix() {
        let id = Uuid::new_v4();
        assert!(blob_path(id, MediaType::Audio, "mp3").contains("/audio/"));
        assert!(blob_path(id, MediaType::Video, "mp4").contains("/video/"));
    }
}
