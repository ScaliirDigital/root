//! The server side of rendering: a pool of persistent worker processes.
//!
//! Why processes at all, even when templates are trusted: a compile cannot be
//! cancelled from inside its own thread. An accidental infinite `#while`, a
//! `#for` over an absurd range, or deep recursion will pin a core and grow the
//! heap, and there is no in-process way to stop it. A stack overflow takes the
//! whole API down with it.
//!
//! Why a *pool* rather than a process per render: spawning costs 10-30 ms per
//! document, and unbounded spawning means memory grows with concurrency until
//! the OOM killer picks a victim -- usually the server, not the worker. A fixed
//! set of long-lived workers makes the ceiling explicit: `size * per-worker`.
//!
//! Worker lifecycle:
//!   - the pool defaults to the parallelism available to this process; operators
//!     can override it with `DOCUMENT_WORKERS`
//!   - a semaphore limits concurrent renders to that same worker count, so excess
//!     work queues instead of spawning unbounded processes
//!   - a job that times out or breaks the protocol kills the worker; the pool
//!     refills lazily on the next request
//!   - a healthy worker is recycled after `MAX_JOBS_PER_WORKER` jobs, because
//!     comemo's cache grows over a process lifetime
//!
//! Both sides live here: the pool above, the worker entrypoint below. The wire
//! format they share is in [`protocol`](super::protocol).

use std::{
    ffi::OsStr,
    io,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::{ExitCode, Stdio},
    sync::{Mutex, OnceLock},
    time::Duration,
};

use crate::core::{
    CompileError, Document, Engine,
    protocol::{
        Job, JobResult, read_frame, read_frame_blocking, write_frame, write_frame_blocking,
    },
};

use tokio::{
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Semaphore,
};

/// Explicit worker-count override.
///
/// Unset means "use the parallelism available to this process". Keeping the
/// override here rather than in the HTTP layer also makes CLI users of the
/// isolated renderer obey the same concurrency ceiling.
const WORKERS_VAR: &str = "DOCUMENT_WORKERS";

/// Recycle a worker after this many jobs.
///
/// Typst caches through comemo, and that cache only grows over a process
/// lifetime. Restarting periodically bounds it without any cache introspection
/// -- the same trick as `max_requests` in php-fpm.
const MAX_JOBS_PER_WORKER: u32 = 500;

/// How long a worker gets to exit on its own before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub wall_clock: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            wall_clock: Duration::from_secs(10),
        }
    }
}

// ---------------------------------------------------------------------------
// Worker handle
// ---------------------------------------------------------------------------

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    jobs: u32,
}

impl Worker {
    /// Spawns one worker from a concrete executable.
    ///
    /// Keeping the process boundary here makes the OS failure deterministic to
    /// test without introducing a fake worker implementation into the pool.
    fn spawn(executable: &Path) -> Result<Self, CompileError> {
        let mut child = Command::new(executable)
            .arg("worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| CompileError::Export(error.to_string()))?;

        let stdin = child.stdin.take().expect("worker stdin is piped");
        let stdout = child.stdout.take().expect("worker stdout is piped");

        Ok(Self {
            child,
            stdin,
            stdout,
            jobs: 0,
        })
    }

    /// One request/response round trip: job in, metadata and PDF out.
    ///
    /// Any error here means the worker is no longer trustworthy -- the caller
    /// drops it rather than returning it to the pool.
    async fn run(&mut self, payload: &[u8]) -> io::Result<(Vec<u8>, Vec<u8>)> {
        let result = run_io(&mut self.stdin, &mut self.stdout, payload).await?;
        self.jobs += 1;
        Ok(result)
    }

    fn is_exhausted(&self) -> bool {
        self.jobs >= MAX_JOBS_PER_WORKER
    }

    /// Lets the worker finish on its own instead of killing it.
    ///
    /// Closing stdin is the shutdown signal: the worker reads EOF and returns.
    /// If it does not do so within the grace period, `kill_on_drop` ends it.
    async fn stop(mut self) {
        drop(self.stdin);
        let _ = tokio::time::timeout(SHUTDOWN_GRACE, self.child.wait()).await;
    }
}

async fn run_io(
    stdin: &mut (dyn tokio::io::AsyncWrite + Unpin + Send),
    stdout: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    payload: &[u8],
) -> io::Result<(Vec<u8>, Vec<u8>)> {
    write_frame(stdin, payload).await?;
    let outcome = read_frame(stdout).await?;
    let pdf = read_frame(stdout).await?;
    Ok((outcome, pdf))
}

// ---------------------------------------------------------------------------
// Pool configuration
// ---------------------------------------------------------------------------

/// Resolves the desired worker count once, when the pool is first needed.
///
/// `DOCUMENT_WORKERS` wins when present. Otherwise the operating system's view
/// of the parallelism available to this process is used. If the platform cannot
/// determine that value, one worker is the conservative fallback.
fn worker_count() -> Result<NonZeroUsize, CompileError> {
    let configured = std::env::var_os(WORKERS_VAR);
    worker_count_from(configured.as_deref(), std::thread::available_parallelism())
}

/// Pure half of [`worker_count`], kept separate so every configuration branch
/// can be tested without mutating process-global environment variables.
fn worker_count_from(
    configured: Option<&OsStr>,
    detected: io::Result<NonZeroUsize>,
) -> Result<NonZeroUsize, CompileError> {
    if let Some(value) = configured {
        let value = value.to_string_lossy();

        return value.parse::<NonZeroUsize>().map_err(|_| {
            CompileError::Configuration(format!(
                "invalid {WORKERS_VAR} `{value}`: expected a positive integer"
            ))
        });
    }

    match detected {
        Ok(count) => Ok(count),
        Err(error) => {
            tracing::warn!(
                %error,
                "available parallelism unavailable -- using one render worker"
            );

            Ok(NonZeroUsize::new(1).expect("one is non-zero"))
        }
    }
}

// ---------------------------------------------------------------------------
// Pool
// ---------------------------------------------------------------------------

struct Pool {
    /// Workers ready to take a job.
    ///
    /// This is deliberately a standard mutex: every critical section is only a
    /// `pop`, `push`, or `take` and no lock survives an `.await`.
    idle: Mutex<Vec<Worker>>,

    /// Maximum number of concurrent renders.
    slots: Semaphore,

    /// Number of worker processes this pool is designed to sustain.
    size: NonZeroUsize,

    /// Executable used to spawn worker subprocesses.
    worker_executable: PathBuf,
}

impl Pool {
    fn new(size: NonZeroUsize, worker_executable: PathBuf) -> Self {
        Self {
            idle: Mutex::new(Vec::with_capacity(size.get())),
            slots: Semaphore::new(size.get()),
            size,
            worker_executable,
        }
    }

    /// Fills the idle pool to its configured size.
    ///
    /// Spawning is all-or-nothing: workers are accumulated separately and only
    /// become visible once every spawn succeeds. If one spawn fails, dropping
    /// the temporary vector kills the workers already created by this attempt.
    ///
    /// Holding the idle lock also makes concurrent warmups idempotent. Warmup is
    /// a startup operation, so briefly holding this synchronous lock while the
    /// child processes are spawned cannot block live render traffic.
    fn warmup(&self) -> Result<usize, CompileError> {
        let mut idle = self.idle.lock().expect("worker pool lock poisoned");
        let missing = self.size.get().saturating_sub(idle.len());

        let mut workers = Vec::with_capacity(missing);

        for _ in 0..missing {
            workers.push(Worker::spawn(&self.worker_executable)?);
        }

        idle.extend(workers);

        Ok(idle.len())
    }

    /// Takes an idle worker, or spawns one when a discarded worker has left a
    /// hole in the pool.
    ///
    /// The caller already owns a semaphore permit, so even the lazy-spawn path
    /// can never grow the number of simultaneously active workers beyond
    /// `size`.
    fn take(&self) -> Result<Worker, CompileError> {
        if let Some(worker) = self.idle.lock().expect("worker pool lock poisoned").pop() {
            return Ok(worker);
        }

        Worker::spawn(&self.worker_executable)
    }

    /// Returns a healthy worker, unless it has served its quota.
    fn give_back(&self, worker: Worker) {
        if worker.is_exhausted() {
            // Dropped here: kill_on_drop reaps the process. The next request
            // that owns a permit but finds no idle worker spawns its replacement.
            return;
        }

        self.idle
            .lock()
            .expect("worker pool lock poisoned")
            .push(worker);
    }

    /// Removes every currently idle worker without holding the lock while the
    /// child processes perform their asynchronous shutdown.
    fn drain(&self) -> Vec<Worker> {
        std::mem::take(&mut *self.idle.lock().expect("worker pool lock poisoned"))
    }
}

/// Initialized on first use so configuration errors can be returned through the
/// normal render/startup path instead of panicking inside a global initializer.
static POOL: OnceLock<Pool> = OnceLock::new();

fn load_pool_config() -> Result<(NonZeroUsize, PathBuf), CompileError> {
    let size = worker_count()?;

    let worker_executable = std::env::current_exe().expect("running process has an executable");

    Ok((size, worker_executable))
}

/// Builds the worker pool. Call once, at startup.
///
/// Configuration errors surface here rather than on the first render, which is
/// where an operator can still do something about them.
///
/// # Errors
///
/// A configuration error for an invalid worker count or an unusable executable.
pub fn initialize() -> Result<(), CompileError> {
    let (size, worker_executable) = load_pool_config()?;

    POOL.get_or_init(|| Pool::new(size, worker_executable));

    Ok(())
}

fn pool() -> &'static Pool {
    POOL.get()
        .expect("the worker pool is initialized at startup")
}

// ---------------------------------------------------------------------------
// Parent side
// ---------------------------------------------------------------------------

/// Configures and pre-spawns the worker pool.
///
/// # Errors
///
/// A configuration error for an invalid worker count, or the spawn error if the
/// pool cannot be filled.
pub fn start() -> Result<usize, CompileError> {
    initialize()?;
    pool().warmup()
}

/// Stops every idle worker.
///
/// Call this only once the server has stopped accepting requests and drained
/// the in-flight ones -- a worker taken from the pool is not in `idle` and
/// would otherwise be killed or returned after this drain.
pub async fn shutdown() {
    shutdown_pool(POOL.get()).await;
}

async fn shutdown_pool(pool: Option<&Pool>) {
    let Some(pool) = pool else {
        return;
    };

    let workers = pool.drain();
    let count = workers.len();

    for worker in workers {
        worker.stop().await;
    }

    tracing::info!(workers = count, "render engine stopped");
}

/// Renders `job` on a pooled worker, bounded by `limits.wall_clock`.
///
/// # Errors
///
/// [`CompileError::ResourceLimit`] if the compile exceeds its wall clock or the
/// worker dies, [`CompileError::Encoding`] if the job or its result cannot be
/// encoded.
pub async fn compile(job: &Job, limits: Limits) -> Result<Document, CompileError> {
    compile_on(pool(), job, limits).await
}

fn timeout_error() -> CompileError {
    tracing::warn!("worker exceeded wall clock, discarding");
    CompileError::ResourceLimit
}

async fn compile_on(pool: &Pool, job: &Job, limits: Limits) -> Result<Document, CompileError> {
    let payload = serde_json::to_vec(job).expect("job is always serializable");

    let _permit = pool
        .slots
        .acquire()
        .await
        .expect("worker pool semaphore is never closed");

    let mut worker = pool.take()?;

    let result = tokio::time::timeout(limits.wall_clock, worker.run(&payload))
        .await
        .map_err(|_| timeout_error())?;

    match result {
        Ok((outcome, pdf)) => {
            pool.give_back(worker);
            decode_outcome(&outcome, pdf)
        }

        Err(error) => {
            tracing::warn!(%error, "worker failed, discarding");
            Err(CompileError::ResourceLimit)
        }
    }
}

fn decode_outcome(outcome: &[u8], pdf: Vec<u8>) -> Result<Document, CompileError> {
    match serde_json::from_slice::<JobResult>(outcome)? {
        JobResult::Ok { warnings } => Ok(Document { pdf, warnings }),
        JobResult::Failed { errors, warnings } => Err(CompileError::Template { errors, warnings }),
    }
}

// ---------------------------------------------------------------------------
// Child side
//
// Everything below runs in a worker process, never in the server. It reads
// framed jobs from stdin until the pipe closes, renders each one, and writes
// the answer to stdout. Diagnostics go to stderr, which the parent inherits, so
// they end up in the server log.
//
// The engine is built once per process and reused across jobs: parsing the
// embedded fonts is the expensive part of a render, and a worker usually sees
// the same template over and over.
// ---------------------------------------------------------------------------

/// Worker entrypoint, reached via the hidden `worker` subcommand.
pub fn run_worker(stdin: &mut dyn io::Read, stdout: &mut dyn io::Write) -> ExitCode {
    let mut engine = Engine::new();

    loop {
        let payload = match read_frame_blocking(stdin) {
            Ok(Some(payload)) => payload,
            Ok(None) => return ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("worker: framing error: {error}");
                return ExitCode::FAILURE;
            }
        };

        let (outcome, pdf) = match execute_job(&mut engine, &payload) {
            Ok(response) => response,
            Err(error) => {
                eprintln!("worker: {error}");
                return ExitCode::FAILURE;
            }
        };

        if write_frame_blocking(stdout, &outcome).is_err()
            || write_frame_blocking(stdout, &pdf).is_err()
        {
            eprintln!("worker: write failed");
            return ExitCode::FAILURE;
        }
    }
}

/// One job, start to finish: decode, render, encode.
///
/// Separated from the loop so each failure is a plain unit test instead of
/// process choreography. The `Err` string is what the worker logs before it
/// gives up on the process.
fn execute_job(engine: &mut Engine, payload: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let job = serde_json::from_slice::<Job>(payload)
        .map_err(|error| format!("undecodable job: {error}"))?;

    let (outcome, pdf) = match engine.render(
        &job.files,
        job.data.as_ref(),
        job.xml.as_deref(),
        &job.options,
    ) {
        Ok(document) => (
            JobResult::Ok {
                warnings: document.warnings,
            },
            document.pdf,
        ),

        // The template is at fault, so this is an answer, not a failure.
        Err(CompileError::Template { errors, warnings }) => {
            (JobResult::Failed { errors, warnings }, Vec::new())
        }

        // Anything else is our problem, not something the caller can fix.
        Err(error) => return Err(format!("render failed: {error}")),
    };

    let encoded = serde_json::to_vec(&outcome).expect("job result is always serializable");

    Ok((encoded, pdf))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn worker_count_accepts_explicit_value() {
        let count = worker_count_from(
            Some(OsStr::new("3")),
            Ok(NonZeroUsize::new(8).expect("eight is non-zero")),
        )
        .expect("explicit worker count should be valid");

        assert_eq!(count.get(), 3);
    }

    #[test]
    fn worker_count_rejects_invalid_explicit_value() {
        let detected = Ok(NonZeroUsize::new(8).expect("eight is non-zero"));

        assert!(worker_count_from(Some(OsStr::new("0")), detected).is_err());

        let detected = Ok(NonZeroUsize::new(8).expect("eight is non-zero"));

        assert!(worker_count_from(Some(OsStr::new("nope")), detected).is_err());
    }

    #[test]
    fn worker_count_uses_detected_parallelism() {
        let detected = NonZeroUsize::new(8).expect("eight is non-zero");
        let count = worker_count_from(None, Ok(detected))
            .expect("detected worker count should be accepted");

        assert_eq!(count, detected);
    }

    #[test]
    fn worker_count_falls_back_to_one() {
        let count = worker_count_from(None, Err(io::Error::other("parallelism unavailable")))
            .expect("parallelism failure should have a safe fallback");

        assert_eq!(count.get(), 1);
    }

    #[test]
    fn worker_spawn_reports_missing_executable() {
        let directory = tempfile::tempdir().expect("failed to create temporary directory");
        let executable = directory.path().join("missing-document-worker");

        assert!(Worker::spawn(&executable).is_err());
    }

    #[test]
    fn rejects_an_undecodable_job() {
        let mut engine = Engine::new();

        let error = execute_job(&mut engine, b"not json").expect_err("undecodable job");

        assert!(error.starts_with("undecodable job:"));
    }

    #[tokio::test]
    async fn exhausted_worker_is_not_returned_to_pool() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let executable = stand_in(&directory);

        let mut worker = Worker::spawn(&executable).expect("spawn worker");
        worker.jobs = MAX_JOBS_PER_WORKER;

        let pool = Pool::new(NonZeroUsize::new(1).expect("one is non-zero"), executable);
        pool.give_back(worker);

        assert!(
            pool.idle
                .lock()
                .expect("worker pool lock poisoned")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn shutdown_without_pool_is_a_noop() {
        shutdown_pool(None).await;
    }

    #[test]
    fn worker_rejects_a_framing_error() {
        let length = u32::try_from(crate::core::protocol::MAX_FRAME_BYTES + 1)
            .expect("oversized frame length fits in u32");
        let header = length.to_le_bytes();
        let mut stdin = header.as_slice();
        let mut stdout = Vec::new();

        assert_eq!(run_worker(&mut stdin, &mut stdout), ExitCode::FAILURE);
    }

    #[test]
    fn worker_reports_a_first_frame_write_error() {
        let mut stdin = framed_executable_job();
        let mut stdout = FailingWriter;

        assert_eq!(run_worker(&mut stdin, &mut stdout), ExitCode::FAILURE);
    }

    #[test]
    fn worker_reports_a_second_frame_write_error() {
        let mut stdin = framed_executable_job();
        let mut stdout = FailOnSecondFlush { flushes: 0 };

        assert_eq!(run_worker(&mut stdin, &mut stdout), ExitCode::FAILURE);
    }

    fn executable_job() -> Job {
        use std::collections::BTreeMap;

        use crate::core::{Files, Pdf, RenderOptions};

        Job {
            files: Files {
                entrypoint: "main.typ".to_owned(),
                content: BTreeMap::from([("main.typ".to_owned(), b"= ok".to_vec())]),
            },
            data: None,
            xml: None,
            options: RenderOptions {
                timestamp: None,
                standard: Pdf::Plain,
            },
        }
    }

    fn framed_executable_job() -> std::io::Cursor<Vec<u8>> {
        let payload = serde_json::to_vec(&executable_job()).expect("encode job");

        let mut wire = Vec::new();
        write_frame_blocking(&mut wire, &payload).expect("frame job");

        std::io::Cursor::new(wire)
    }

    struct FailingWriter;

    impl io::Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("write failed"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct FailOnSecondFlush {
        flushes: usize,
    }

    impl io::Write for FailOnSecondFlush {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;

            if self.flushes == 2 {
                return Err(io::Error::other("flush failed"));
            }

            Ok(())
        }
    }

    /// A worker stand-in: ignores its arguments, consumes stdin, exits on EOF.
    ///
    /// Never `current_exe()` — the test binary reads `worker` as a test filter
    /// and spawns itself. Never `/bin/cat` either: it rejects the argument and
    /// its complaint lands in the test output, because worker stderr is
    /// inherited on purpose.
    fn stand_in(directory: &tempfile::TempDir) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = directory.path().join("stand-in");

        std::fs::write(&path, "#!/bin/sh\nexec cat > /dev/null\n").expect("write stand-in");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make stand-in executable");

        path
    }

    fn successful_stand_in(directory: &tempfile::TempDir) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = directory.path().join("successful-stand-in");

        std::fs::write(
            &path,
            "#!/bin/sh\n\
     dd bs=1 count=5 of=/dev/null 2>/dev/null\n\
     printf '\\026\\000\\000\\000{\"Ok\":{\"warnings\":[]}}\\003\\000\\000\\000pdf'\n",
        )
        .expect("write successful stand-in");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make successful stand-in executable");

        path
    }

    #[tokio::test]
    async fn compile_returns_a_successful_document() {
        let directory = tempfile::tempdir().expect("temporary directory");

        let pool = Pool::new(
            NonZeroUsize::new(1).expect("one is non-zero"),
            successful_stand_in(&directory),
        );

        let document = compile_on(
            &pool,
            &executable_job(),
            Limits {
                wall_clock: Duration::from_secs(1),
            },
        )
        .await
        .expect("isolated compile should succeed");

        assert_eq!(document.pdf, b"pdf");
        assert!(document.warnings.is_empty());

        assert_eq!(
            pool.idle.lock().expect("worker pool lock poisoned").len(),
            1,
            "successful worker should be returned to the pool",
        );

        shutdown_pool(Some(&pool)).await;
    }

    #[test]
    fn failing_writer_can_flush() {
        let mut writer = FailingWriter;

        writer.flush().expect("flush");
    }

    #[test]
    fn worker_rejects_an_undecodable_job() {
        let mut wire = Vec::new();
        write_frame_blocking(&mut wire, b"not json").expect("frame invalid job");

        let mut stdin = std::io::Cursor::new(wire);
        let mut stdout = Vec::new();

        assert_eq!(run_worker(&mut stdin, &mut stdout), ExitCode::FAILURE);

        assert!(stdout.is_empty());
    }

    #[test]
    fn execute_job_reports_an_engine_failure() {
        let mut engine = Engine::new();
        let mut job = executable_job();

        job.files
            .content
            .insert("main.typ".to_owned(), vec![0xff, 0xfe, 0xfd]);

        let payload = serde_json::to_vec(&job).expect("encode job");

        let error =
            execute_job(&mut engine, &payload).expect_err("invalid source should fail the worker");

        assert!(error.starts_with("render failed:"));
    }

    #[test]
    fn writer_can_fail_on_second_flush() {
        let mut writer = FailOnSecondFlush { flushes: 0 };

        writer.flush().expect("first flush");
        assert!(writer.flush().is_err());
    }

    #[test]
    fn decode_outcome_accepts_success() {
        let outcome = serde_json::to_vec(&JobResult::Ok {
            warnings: vec!["warning".to_owned()],
        })
        .expect("encode outcome");

        let document = decode_outcome(&outcome, b"pdf".to_vec()).expect("successful outcome");

        assert_eq!(document.pdf, b"pdf");
        assert_eq!(document.warnings, ["warning"]);
    }

    #[test]
    fn decode_outcome_maps_template_failure() {
        let outcome = serde_json::to_vec(&JobResult::Failed {
            errors: vec!["broken".to_owned()],
            warnings: vec!["warning".to_owned()],
        })
        .expect("encode outcome");

        let error = decode_outcome(&outcome, Vec::new())
            .err()
            .expect("expected template error");

        assert_eq!(
            std::mem::discriminant(&error),
            std::mem::discriminant(&CompileError::Template {
                errors: Vec::new(),
                warnings: Vec::new(),
            }),
        );
    }

    #[test]
    fn decode_outcome_rejects_invalid_response() {
        assert!(decode_outcome(b"not json", Vec::new()).is_err());
    }

    #[tokio::test]
    async fn run_io_reports_write_error() {
        let (mut stdin, peer) = tokio::io::duplex(64);
        drop(peer);

        let mut stdout = tokio::io::empty();

        let error = run_io(&mut stdin, &mut stdout, b"job")
            .await
            .expect_err("write must fail");

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[tokio::test]
    async fn run_io_reports_first_read_error() {
        let mut stdin = tokio::io::sink();
        let mut stdout = tokio::io::empty();

        assert!(run_io(&mut stdin, &mut stdout, b"job").await.is_err());
    }

    #[tokio::test]
    async fn run_io_reports_second_read_error() {
        let mut stdin = tokio::io::sink();
        let (mut writer, mut stdout) = tokio::io::duplex(64);

        write_frame(&mut writer, b"outcome")
            .await
            .expect("write outcome");

        drop(writer);

        assert!(run_io(&mut stdin, &mut stdout, b"job").await.is_err());
    }

    #[test]
    fn warmup_reports_worker_spawn_failure() {
        let directory = tempfile::tempdir().expect("temporary directory");

        let pool = Pool::new(
            NonZeroUsize::new(1).expect("one is non-zero"),
            directory.path().join("missing-worker"),
        );

        assert!(pool.warmup().is_err());
    }

    #[test]
    fn take_reports_worker_spawn_failure() {
        let directory = tempfile::tempdir().expect("temporary directory");

        let pool = Pool::new(
            NonZeroUsize::new(1).expect("one is non-zero"),
            directory.path().join("missing-worker"),
        );

        assert!(pool.take().is_err());
    }

    #[tokio::test]
    async fn compile_reports_worker_spawn_failure() {
        let directory = tempfile::tempdir().expect("temporary directory");

        let pool = Pool::new(
            NonZeroUsize::new(1).expect("one is non-zero"),
            directory.path().join("missing-worker"),
        );

        let job = executable_job();

        assert!(compile_on(&pool, &job, Limits::default()).await.is_err());
    }

    #[tokio::test]
    async fn shutdown_stops_idle_workers() {
        let directory = tempfile::tempdir().expect("temporary directory");

        let pool = Pool::new(
            NonZeroUsize::new(1).expect("one is non-zero"),
            stand_in(&directory),
        );

        pool.warmup().expect("warm the pool");
        shutdown_pool(Some(&pool)).await;

        assert!(pool.drain().is_empty());
    }

    #[test]
    fn worker_exits_cleanly_on_eof() {
        let mut stdin = std::io::empty();
        let mut stdout = Vec::new();

        let code = run_worker(&mut stdin, &mut stdout);

        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[test]
    fn timeout_maps_to_resource_limit() {
        let error = timeout_error();

        assert_eq!(
            std::mem::discriminant(&error),
            std::mem::discriminant(&CompileError::ResourceLimit),
        );
    }
    #[tokio::test]
    async fn global_pool_lifecycle() {
        initialize().expect("initialize the global pool");
        initialize().expect("initializing twice is harmless");

        let workers = start().expect("start the pool");

        assert_eq!(workers, pool().size.get());

        assert_eq!(workers, pool().size.get());

        let result = compile(
            &executable_job(),
            Limits {
                wall_clock: Duration::from_secs(1),
            },
        )
        .await;

        assert!(result.is_err());

        shutdown().await;

        assert!(pool().drain().is_empty());
    }

    #[test]
    fn run_worker_completes_a_successful_job() {
        let mut stdin = framed_executable_job();
        let mut stdout = Vec::new();

        let code = run_worker(&mut stdin, &mut stdout);

        assert_eq!(code, ExitCode::SUCCESS);
        assert!(!stdout.is_empty());
    }
}
