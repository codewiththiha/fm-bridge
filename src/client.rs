//! Process management and the NDJSON IPC loop.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_core::Stream;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::error::{Error, Result};
use crate::types::*;

/// Default name of the helper binary produced by `swift build`.
const BINARY_NAME: &str = "FMBridge";

/// Environment variable consulted by [`Bridge::from_env`].
pub const BINARY_ENV_VAR: &str = "FM_BRIDGE_BIN";

/// How many helper processes a fresh [`Bridge`] runs at once.
///
/// One, deliberately: the on-device model is a shared, memory-hungry resource,
/// so fanning out is something you opt into with
/// [`Bridge::max_concurrency`] rather than something you get by accident.
pub const DEFAULT_MAX_CONCURRENCY: usize = 1;

/// A handle to the Swift helper binary.
///
/// Cloning is cheap, and **clones share one concurrency budget**: the limit set
/// by [`max_concurrency`](Self::max_concurrency) applies across every clone, so
/// handing a clone to each task still caps the number of helper processes
/// running at once. Call `max_concurrency` on a clone to give it a separate
/// budget instead.
///
/// Each request spawns its own short-lived helper process, so concurrent calls
/// never share a session and responses can never be crossed between callers.
#[derive(Clone, Debug)]
pub struct Bridge {
    binary: PathBuf,
    timeout: Option<Duration>,
    limit: usize,
    /// Shared across clones so the limit is global rather than per-handle.
    limiter: Arc<Semaphore>,
}

impl Bridge {
    /// Points the bridge at a helper binary.
    ///
    /// Concurrency defaults to [`DEFAULT_MAX_CONCURRENCY`] (one request at a
    /// time); use [`max_concurrency`](Self::max_concurrency) to raise it.
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            timeout: Some(Duration::from_secs(300)),
            limit: DEFAULT_MAX_CONCURRENCY,
            limiter: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENCY)),
        }
    }

    /// Reads the helper path from the `FM_BRIDGE_BIN` environment variable.
    pub fn from_env() -> Result<Self> {
        Self::from_env_value(std::env::var_os(BINARY_ENV_VAR))
    }

    /// The body of [`from_env`](Self::from_env), split out so it can be tested
    /// by passing a value in rather than mutating the process environment
    /// (which is `unsafe` in edition 2024, and racy in a threaded test binary).
    pub(crate) fn from_env_value(raw: Option<std::ffi::OsString>) -> Result<Self> {
        let path = raw.ok_or_else(|| Error::Protocol(format!("{BINARY_ENV_VAR} is not set")))?;
        if path.is_empty() {
            return Err(Error::Protocol(format!(
                "{BINARY_ENV_VAR} is set but empty"
            )));
        }
        Ok(Self::new(PathBuf::from(path)))
    }

    /// Locates a helper embedded in the running `.app` bundle.
    ///
    /// **This is the constructor to use in shipped applications.** It resolves
    /// the helper strictly relative to the running executable, checking
    /// `Contents/MacOS/FMBridge` and then `Contents/Resources/FMBridge`.
    ///
    /// Unlike [`discover`](Self::discover) it never consults `PATH`, the
    /// working directory, or `FM_BRIDGE_BIN` — all of which can be
    /// influenced by whoever launches your app, and none of which belong in a
    /// code path that spawns an executable.
    ///
    /// ```text
    /// YourApp.app/
    /// └── Contents/
    ///     ├── MacOS/
    ///     │   ├── YourApp        ← the running executable
    ///     │   └── FMBridge      ← found here
    ///     └── Resources/
    ///         └── FMBridge      ← or here
    /// ```
    pub fn bundled() -> Result<Self> {
        let exe = std::env::current_exe().map_err(Error::Io)?;
        let dir = exe
            .parent()
            .ok_or_else(|| Error::Protocol("executable has no parent directory".into()))?;
        resolve_bundled(dir).map(Self::new)
    }

    /// Locates the helper automatically, for development and CLI use.
    ///
    /// Checks, in order: `FM_BRIDGE_BIN`, the `swift/.build` directories
    /// under the working directory, the directory holding the running
    /// executable, and finally `PATH`.
    ///
    /// Prefer [`bundled`](Self::bundled) in a shipped `.app`; this function
    /// trusts environment state that an end user's machine may not control.
    pub fn discover() -> Result<Self> {
        if let Ok(bridge) = Self::from_env() {
            if bridge.binary.exists() {
                return Ok(bridge);
            }
        }

        let mut candidates: Vec<PathBuf> = Vec::new();

        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("swift/.build/release").join(BINARY_NAME));
            candidates.push(cwd.join("swift/.build/debug").join(BINARY_NAME));
            candidates.push(cwd.join(BINARY_NAME));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join(BINARY_NAME));
            }
        }
        if let Some(path_var) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path_var).map(|dir| dir.join(BINARY_NAME)));
        }

        candidates
            .into_iter()
            .find(|candidate| candidate.is_file())
            .map(Self::new)
            .ok_or_else(|| Error::BinaryNotFound(PathBuf::from(BINARY_NAME)))
    }

    /// Path to the helper binary.
    pub fn binary_path(&self) -> &Path {
        &self.binary
    }

    /// Sets a wall-clock timeout for each request. `None` disables it.
    ///
    /// The clock starts when you call [`complete`](Self::complete) or poll
    /// [`stream`](Self::stream) — *not* when a helper process finally starts.
    /// Time spent queued behind the concurrency limit therefore counts against
    /// the timeout, so a saturated bridge reports [`Error::Timeout`] instead of
    /// making callers wait indefinitely.
    pub fn timeout(mut self, timeout: impl Into<Option<Duration>>) -> Self {
        self.timeout = timeout.into();
        self
    }

    /// Limits how many requests may run at the same time.
    ///
    /// Every request — [`complete`](Self::complete),
    /// [`stream`](Self::stream), and [`check_availability`](Self::check_availability)
    /// — takes a slot for as long as it holds a helper process, and releases it
    /// when the response finishes, errors, or the future/stream is dropped.
    /// Calls beyond the limit **queue** rather than fail, and are admitted in
    /// roughly the order they arrived.
    ///
    /// The default is [`DEFAULT_MAX_CONCURRENCY`] (1). A limit of `0` is
    /// treated as `1`, since a bridge that can never run anything is never what
    /// the caller meant.
    ///
    /// The budget is shared by every clone made *after* this call:
    ///
    /// ```
    /// use fm_bridge::Bridge;
    ///
    /// let bridge = Bridge::new("/path/to/FMBridge").max_concurrency(4);
    /// let worker = bridge.clone(); // same 4-slot budget as `bridge`
    ///
    /// assert_eq!(bridge.max_concurrency_limit(), 4);
    /// assert_eq!(worker.available_slots(), 4);
    /// ```
    ///
    /// # Choosing a value
    ///
    /// Each slot is a separate helper process holding its own session against a
    /// single shared on-device model, so throughput does not scale linearly and
    /// large values mostly buy memory pressure. Small numbers (2–4) are a
    /// reasonable ceiling; keep the default of 1 if you want strict serial
    /// behaviour. If the model itself rejects overlapping work it surfaces as a
    /// retryable [`Error::Generation`] containing "already responding".
    pub fn max_concurrency(mut self, limit: usize) -> Self {
        let limit = limit.max(1);
        self.limit = limit;
        self.limiter = Arc::new(Semaphore::new(limit));
        self
    }

    /// The configured concurrency limit.
    pub fn max_concurrency_limit(&self) -> usize {
        self.limit
    }

    /// How many slots are free right now.
    ///
    /// Purely informational — by the time you act on it another task may have
    /// taken a slot. Useful for metrics and backpressure heuristics.
    pub fn available_slots(&self) -> usize {
        self.limiter.available_permits()
    }

    /// The instant at which a request starting now runs out of time.
    fn deadline(&self) -> Option<tokio::time::Instant> {
        self.timeout.map(|d| tokio::time::Instant::now() + d)
    }

    /// Checks whether Apple Intelligence is usable, without generating anything.
    ///
    /// Returns `Ok(())` when the model is ready, or
    /// [`Error::ModelUnavailable`] describing why it is not.
    ///
    /// Takes a concurrency slot like any other request, so a probe issued while
    /// the bridge is saturated waits its turn instead of adding an extra
    /// process.
    pub async fn check_availability(&self) -> Result<()> {
        self.ensure_binary()?;
        let deadline = self.deadline();
        let _permit = acquire_slot(&self.limiter, deadline, self.timeout).await?;

        let mut child = Command::new(&self.binary)
            .arg("--probe")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| Error::Spawn {
                path: self.binary.clone(),
                source,
            })?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let mut lines = BufReader::new(stdout).lines();
        let mut failure = None;

        while let Some(line) = lines.next_line().await? {
            match parse_line(&line) {
                Ok(_) => {}
                Err(error) => failure = Some(error),
            }
            if line.contains("\"ready\"") {
                break;
            }
        }

        let _ = child.wait().await;
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Runs a request to completion and returns the whole response at once.
    ///
    /// Works for both plain text and structured output; inspect
    /// [`Completion::text`] or [`Completion::structured`] accordingly.
    /// Requests beyond the [concurrency limit](Self::max_concurrency) wait for
    /// a free slot; that wait counts toward the configured timeout.
    pub async fn complete(&self, request: Request) -> Result<Completion> {
        request.validate()?;
        self.ensure_binary()?;

        // One deadline spans both the queue wait and the generation itself, so
        // a backlog can never stretch the total time past the timeout.
        let deadline = self.deadline();
        let _permit = acquire_slot(&self.limiter, deadline, self.timeout).await?;

        let future = self.complete_inner(&request);
        match (deadline, self.timeout) {
            (Some(deadline), Some(duration)) => tokio::time::timeout_at(deadline, future)
                .await
                .map_err(|_| Error::Timeout(duration))?,
            _ => future.await,
        }
    }

    async fn complete_inner(&self, request: &Request) -> Result<Completion> {
        // Structured requests are collected in one shot; snapshots would only
        // be discarded here, so ask the helper not to produce them.
        let stream_mode = request.schema.is_none();
        let wire = WireRequest::new(request, stream_mode);
        let mut session = Session::spawn(&self.binary, &wire).await?;

        let mut completion = Completion::default();
        let mut saw_done = false;

        while let Some(line) = session.next_line().await? {
            match parse_line(&line)? {
                Some(StreamEvent::Delta(delta)) => completion.text.push_str(&delta),
                Some(StreamEvent::Snapshot(_)) => {}
                Some(StreamEvent::Structured(value)) => completion.structured = Some(value),
                Some(StreamEvent::Done(usage)) => {
                    completion.usage = usage;
                    saw_done = true;
                    break;
                }
                None => {}
            }
        }

        session.finish(saw_done).await?;
        Ok(completion)
    }

    /// Runs a request and yields events as they are produced.
    ///
    /// Text generation streams incrementally. Structured generation also
    /// streams when [`Request::stream_structured`] is enabled, emitting
    /// [`StreamEvent::Snapshot`] values before the final
    /// [`StreamEvent::Structured`].
    ///
    /// Dropping the returned stream kills the helper process immediately and
    /// releases its [concurrency slot](Self::max_concurrency).
    ///
    /// The slot is taken when the stream is first polled, not when this
    /// function returns, so building a stream you never poll costs nothing.
    /// Waiting for a slot counts toward the configured timeout.
    pub fn stream(&self, request: Request) -> impl Stream<Item = Result<StreamEvent>> + Send {
        let binary = self.binary.clone();
        let timeout = self.timeout;
        let limiter = Arc::clone(&self.limiter);

        async_stream::try_stream! {
            request.validate()?;
            // Streaming a schema-constrained response only makes sense when the
            // caller has opted into partial snapshots; otherwise they almost
            // certainly wanted `complete`, and silently emitting a single
            // structured event at the end would hide the mistake.
            if request.schema.is_some() && !request.stream_structured {
                Err(Error::BadRequest(
                    "structured requests cannot be streamed unless \
                     `Request::stream_structured(true)` is set; use `complete` instead"
                        .into(),
                ))?;
            }
            if !binary.is_file() {
                Err(Error::BinaryNotFound(binary.clone()))?;
            }

            // Both the queue wait and the generation share one deadline. The
            // permit is held by this generator, so it is released when the
            // stream completes *or* when the caller drops it early.
            let deadline = timeout.map(|d| tokio::time::Instant::now() + d);
            let _permit = acquire_slot(&limiter, deadline, timeout).await?;

            let wire = WireRequest::new(&request, true);
            let mut session = Session::spawn(&binary, &wire).await?;
            let mut saw_done = false;

            loop {
                let line = match deadline {
                    Some(deadline) => {
                        match tokio::time::timeout_at(deadline, session.next_line()).await {
                            Ok(result) => result?,
                            Err(_) => {
                                Err(Error::Timeout(timeout.expect("deadline implies timeout")))?;
                                unreachable!()
                            }
                        }
                    }
                    None => session.next_line().await?,
                };

                let Some(line) = line else { break };

                if let Some(event) = parse_line(&line)? {
                    let done = matches!(event, StreamEvent::Done(_));
                    yield event;
                    if done {
                        saw_done = true;
                        break;
                    }
                }
            }

            session.finish(saw_done).await?;
        }
    }

    fn ensure_binary(&self) -> Result<()> {
        if self.binary.is_file() {
            Ok(())
        } else {
            Err(Error::BinaryNotFound(self.binary.clone()))
        }
    }
}

/// Waits for a concurrency slot, giving up at `deadline`.
///
/// Returns an *owned* permit so it can be held by a spawned task or an
/// `async_stream` generator; dropping it frees the slot, which is what makes an
/// abandoned stream release its budget without any explicit cleanup.
///
/// Queued time is charged against the request timeout: if the bridge is
/// saturated for longer than that, the caller gets [`Error::Timeout`] rather
/// than waiting forever.
async fn acquire_slot(
    limiter: &Arc<Semaphore>,
    deadline: Option<tokio::time::Instant>,
    timeout: Option<Duration>,
) -> Result<OwnedSemaphorePermit> {
    let limiter = Arc::clone(limiter);
    // `acquire_owned` errors only if the semaphore is closed, which this crate
    // never does — report it as a protocol fault rather than panicking.
    let acquire = async move {
        limiter
            .acquire_owned()
            .await
            .map_err(|_| Error::Protocol("concurrency limiter was closed".into()))
    };

    match deadline {
        Some(deadline) => match tokio::time::timeout_at(deadline, acquire).await {
            Ok(permit) => permit,
            Err(_) => Err(Error::Timeout(
                timeout.unwrap_or_else(|| Duration::from_secs(0)),
            )),
        },
        None => acquire.await,
    }
}

/// Owns a spawned helper process plus its piped streams.
struct Session {
    child: Child,
    stdout: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    stderr: Arc<Mutex<String>>,
    stderr_task: tokio::task::JoinHandle<()>,
}

impl Session {
    async fn spawn(binary: &Path, request: &WireRequest<'_>) -> Result<Self> {
        #[cfg(feature = "tracing")]
        tracing::debug!(binary = %binary.display(), "spawning FMBridge");

        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Guarantees the helper dies with us rather than lingering if the
            // caller drops the future or the stream part-way through.
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| Error::Spawn {
                path: binary.to_path_buf(),
                source,
            })?;

        let mut payload = serde_json::to_vec(request)?;
        payload.push(b'\n');

        let mut stdin = child.stdin.take().expect("stdin was piped");
        // Write on a task: a large prompt can fill the pipe buffer, and the
        // helper will not drain it until it starts reading, so writing inline
        // could deadlock against a helper that writes to stdout first.
        let write_task = tokio::spawn(async move {
            stdin.write_all(&payload).await?;
            stdin.flush().await?;
            // Closing stdin signals end-of-request to the helper.
            stdin.shutdown().await
        });

        let stdout = child.stdout.take().expect("stdout was piped");
        let mut stderr_pipe = child.stderr.take().expect("stderr was piped");

        // Drain stderr continuously; otherwise a chatty helper can block on a
        // full stderr pipe and deadlock the whole exchange.
        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_sink = Arc::clone(&stderr);
        let stderr_task = tokio::spawn(async move {
            let mut buffer = Vec::new();
            if stderr_pipe.read_to_end(&mut buffer).await.is_ok() {
                let text = String::from_utf8_lossy(&buffer).into_owned();
                stderr_sink.lock().await.push_str(&text);
            }
        });

        match write_task.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                // The helper exited before reading the request; the real reason
                // will be on stderr and is reported by `finish`.
            }
            Ok(Err(error)) => return Err(Error::Io(error)),
            Err(join_error) => {
                return Err(Error::Protocol(format!(
                    "stdin writer panicked: {join_error}"
                )));
            }
        }

        Ok(Self {
            child,
            stdout: BufReader::new(stdout).lines(),
            stderr,
            stderr_task,
        })
    }

    async fn next_line(&mut self) -> Result<Option<String>> {
        Ok(self.stdout.next_line().await?)
    }

    /// Reaps the process and turns a non-zero exit into a typed error.
    async fn finish(mut self, saw_done: bool) -> Result<()> {
        let status = self.child.wait().await?;
        let _ = self.stderr_task.await;
        let stderr = self.stderr.lock().await.clone();

        if !status.success() {
            // A helper that already reported a typed error on stdout has been
            // surfaced upstream; only report the raw exit if nothing was.
            return Err(Error::ProcessFailed {
                status: describe_status(&status),
                stderr,
            });
        }

        if !saw_done {
            return Err(Error::Protocol(format!(
                "FMBridge exited without sending a completion event{}",
                if stderr.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", stderr.trim())
                }
            )));
        }

        Ok(())
    }
}

/// Resolves an embedded helper relative to the directory holding the running
/// executable, following the layout Apple prescribes for bundled tools.
///
/// Split out from [`Bridge::bundled`] so it can be tested against a
/// synthetic bundle without relocating the test binary itself.
pub(crate) fn resolve_bundled(exe_dir: &Path) -> Result<PathBuf> {
    let candidates = [
        // Contents/MacOS/FMBridge — a Copy Files phase with destination
        // "Executables", which is what Apple recommends.
        exe_dir.join(BINARY_NAME),
        // Contents/MacOS/../Resources/FMBridge — destination "Resources".
        exe_dir.join("..").join("Resources").join(BINARY_NAME),
    ];

    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .map(|candidate| {
            // Resolve `..` and any symlinks so the stored path is the real one;
            // fall back to the literal path if canonicalization fails.
            std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.clone())
        })
        .ok_or_else(|| Error::BinaryNotFound(exe_dir.join(BINARY_NAME)))
}

fn describe_status(status: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("killed by signal {signal}");
        }
    }
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "unknown".to_string(),
    }
}

/// Parses one NDJSON line into an event.
///
/// Unrecognised or non-JSON lines yield `Ok(None)` so stray helper output can
/// never corrupt a response.
pub(crate) fn parse_line(line: &str) -> Result<Option<StreamEvent>> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let Some(object) = value.as_object() else {
        return Ok(None);
    };

    if let Some(message) = object.get("error").and_then(|e| e.as_str()) {
        let code = object
            .get("code")
            .and_then(|c| c.as_str())
            .unwrap_or("generation_failed");
        let reason = object.get("reason").and_then(|r| r.as_str());
        return Err(Error::from_code(code, message.to_string(), reason));
    }

    if object.get("done").and_then(serde_json::Value::as_bool) == Some(true) {
        let usage = object
            .get("usage")
            .map(|usage| Usage {
                prompt_tokens: field_u32(usage, "promptTokens"),
                completion_tokens: field_u32(usage, "completionTokens"),
            })
            .unwrap_or_default();
        return Ok(Some(StreamEvent::Done(usage)));
    }

    if let Some(structured) = object.get("structured") {
        return Ok(Some(StreamEvent::Structured(structured.clone())));
    }

    if let Some(snapshot) = object.get("snapshot") {
        return Ok(Some(StreamEvent::Snapshot(snapshot.clone())));
    }

    if let Some(delta) = object.get("delta").and_then(|d| d.as_str()) {
        return Ok(Some(StreamEvent::Delta(delta.to_string())));
    }

    // `ready` (from --probe) and anything unknown are intentionally ignored.
    Ok(None)
}

fn field_u32(value: &serde_json::Value, key: &str) -> u32 {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32
}
