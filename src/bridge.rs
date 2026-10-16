use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_core::Stream;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use crate::error::{Error, Result};
use crate::locate;
use crate::request::Request;
use crate::response::{Availability, Completion, StreamEvent};
use crate::session::Session;
use crate::wire::{Line, WireRequest};

/// Requests a fresh bridge runs at once; the model is one shared resource.
pub const DEFAULT_MAX_CONCURRENCY: usize = 1;

/// Per-request limit a fresh bridge applies, queueing included.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// A handle to the `FMBridge` helper; clones share one concurrency budget.
#[derive(Clone, Debug)]
pub struct Bridge {
    binary: PathBuf,
    timeout: Option<Duration>,
    limit: usize,
    slots: Arc<Semaphore>,
}

impl Bridge {
    /// A bridge running the helper at `binary`.
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            timeout: Some(DEFAULT_TIMEOUT),
            limit: DEFAULT_MAX_CONCURRENCY,
            slots: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENCY)),
        }
    }

    /// The helper named by `FM_BRIDGE_BIN`.
    pub fn from_env() -> Result<Self> {
        locate::from_env_value(std::env::var_os(locate::BINARY_ENV_VAR)).map(Self::new)
    }

    /// The helper inside the running `.app`; the one to ship.
    pub fn bundled() -> Result<Self> {
        locate::bundled().map(Self::new)
    }

    /// Searches the environment, `swift/.build` and `PATH`; for development.
    pub fn discover() -> Result<Self> {
        locate::discover().map(Self::new)
    }

    /// The helper's path.
    pub fn binary_path(&self) -> &Path {
        &self.binary
    }

    /// Limits each request, queueing included; `None` waits forever.
    pub fn timeout(mut self, timeout: impl Into<Option<Duration>>) -> Self {
        self.timeout = timeout.into();
        self
    }

    /// Runs up to `limit` at once; later clones share this budget.
    pub fn max_concurrency(mut self, limit: usize) -> Self {
        self.limit = limit.max(1);
        self.slots = Arc::new(Semaphore::new(self.limit));
        self
    }

    /// The concurrency limit.
    pub fn max_concurrency_limit(&self) -> usize {
        self.limit
    }

    /// Free slots right now; informational only.
    pub fn available_slots(&self) -> usize {
        self.slots.available_permits()
    }

    fn deadline(&self) -> Option<Instant> {
        self.timeout.map(|d| Instant::now() + d)
    }

    async fn slot(&self, deadline: Option<Instant>) -> Result<OwnedSemaphorePermit> {
        if !self.binary.is_file() {
            return Err(Error::BinaryNotFound(self.binary.clone()));
        }
        let acquire = Arc::clone(&self.slots).acquire_owned();
        let permit = match deadline {
            Some(at) => tokio::time::timeout_at(at, acquire)
                .await
                .map_err(|_| self.timed_out())?,
            None => acquire.await,
        };
        permit.map_err(|_| Error::Protocol("concurrency limiter closed".into()))
    }

    fn timed_out(&self) -> Error {
        Error::Timeout(self.timeout.unwrap_or_default())
    }

    async fn session(
        &self,
        args: &[&str],
        input: Option<Vec<u8>>,
        deadline: Option<Instant>,
    ) -> Result<Session> {
        let timeout = self.timeout.unwrap_or_default();
        Session::spawn(&self.binary, args, input, deadline, timeout).await
    }

    /// Asks whether the model can serve, without generating.
    pub async fn check_availability(&self) -> Result<Availability> {
        let deadline = self.deadline();
        let _slot = self.slot(deadline).await?;
        let mut session = self.session(&["--probe"], None, deadline).await?;
        let mut ready = None;
        while let Some(line) = session.next().await? {
            if let Line::Ready(model) = line {
                ready = Some(model);
                break;
            }
        }
        session.finish(ready.is_some()).await?;
        ready.ok_or_else(|| Error::Protocol("probe sent no reply".into()))
    }

    /// Runs `request` and returns the whole answer.
    pub async fn complete(&self, request: Request) -> Result<Completion> {
        request.validate()?;
        let deadline = self.deadline();
        let _slot = self.slot(deadline).await?;
        // A schema is answered in one piece; snapshots would be dropped.
        let wire = WireRequest::new(&request, request.schema.is_none());
        let input = serde_json::to_vec(&wire)?;
        let mut session = self.session(&[], Some(input), deadline).await?;
        let mut completion = Completion::default();
        let mut ended = false;
        while let Some(line) = session.next().await? {
            match line {
                Line::Event(StreamEvent::Delta(text)) => completion.text.push_str(&text),
                Line::Event(StreamEvent::Structured(value)) => completion.structured = Some(value),
                Line::Event(StreamEvent::Done(usage)) => {
                    completion.usage = usage;
                    ended = true;
                    break;
                }
                _ => {}
            }
        }
        session.finish(ended).await?;
        Ok(completion)
    }

    /// Yields events as they come; dropping the stream kills the helper.
    pub fn stream(
        &self,
        request: Request,
    ) -> impl Stream<Item = Result<StreamEvent>> + Send + 'static {
        let bridge = self.clone();
        async_stream::try_stream! {
            request.validate()?;
            if request.schema.is_some() && !request.stream_structured {
                Err(Error::BadRequest("structured streaming needs stream_structured(true)".into()))?;
            }
            let deadline = bridge.deadline();
            let _slot = bridge.slot(deadline).await?;
            let input = serde_json::to_vec(&WireRequest::new(&request, true))?;
            let mut session = bridge.session(&[], Some(input), deadline).await?;
            let mut ended = false;
            while let Some(line) = session.next().await? {
                if let Line::Event(event) = line {
                    ended = matches!(event, StreamEvent::Done(_));
                    yield event;
                    if ended {
                        break;
                    }
                }
            }
            session.finish(ended).await?;
        }
    }
}
