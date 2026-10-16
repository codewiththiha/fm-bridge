use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::error::{Error, Result};
use crate::wire::{Line, parse_line};

/// Stderr kept for error reports; the rest is dropped.
const STDERR_CAP: u64 = 16 * 1024;

/// One helper process, killed when dropped.
pub(crate) struct Session {
    child: Child,
    stdout: Lines<BufReader<ChildStdout>>,
    stderr: JoinHandle<String>,
    deadline: Option<Instant>,
    timeout: std::time::Duration,
}

impl Session {
    /// Starts the helper; `input` is the request line, or none for a probe.
    pub(crate) async fn spawn(
        binary: &Path,
        args: &[&str],
        input: Option<Vec<u8>>,
        deadline: Option<Instant>,
        timeout: std::time::Duration,
    ) -> Result<Self> {
        let stdin = if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        };
        let mut child = Command::new(binary)
            .args(args)
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| Error::Spawn {
                path: PathBuf::from(binary),
                source,
            })?;
        let stdout = child.stdout.take().ok_or_else(|| piped("stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| piped("stderr"))?;
        // Drained on a task so a chatty helper can't block on a full pipe.
        let stderr = tokio::spawn(async move {
            let mut text = String::new();
            let _ = stderr.take(STDERR_CAP).read_to_string(&mut text).await;
            text
        });
        if let Some(mut payload) = input {
            payload.push(b'\n');
            let mut stdin = child.stdin.take().ok_or_else(|| piped("stdin"))?;
            let written = async {
                stdin.write_all(&payload).await?;
                stdin.shutdown().await
            };
            match written.await {
                // The helper quit before reading; `finish` reports why.
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                other => other?,
            }
        }
        Ok(Self {
            child,
            stdout: BufReader::new(stdout).lines(),
            stderr,
            deadline,
            timeout,
        })
    }

    /// The next meaningful line, or none at end of output.
    pub(crate) async fn next(&mut self) -> Result<Option<Line>> {
        loop {
            let read = self.stdout.next_line();
            let line = match self.deadline {
                Some(at) => tokio::time::timeout_at(at, read)
                    .await
                    .map_err(|_| Error::Timeout(self.timeout))??,
                None => read.await?,
            };
            match line {
                None => return Ok(None),
                Some(text) => match parse_line(&text)? {
                    Line::Noise => {}
                    meaningful => return Ok(Some(meaningful)),
                },
            }
        }
    }

    /// Reaps the process; a failed exit or a missing end becomes an error.
    pub(crate) async fn finish(mut self, ended: bool) -> Result<()> {
        let status = self.child.wait().await?;
        let stderr = (&mut self.stderr).await.unwrap_or_default();
        if !status.success() {
            return Err(Error::ProcessFailed {
                status: describe(status),
                stderr,
            });
        }
        if !ended {
            let detail = stderr.trim();
            return Err(Error::Protocol(format!(
                "FMBridge exited without finishing{}{detail}",
                if detail.is_empty() { "" } else { ": " }
            )));
        }
        Ok(())
    }
}

fn piped(stream: &str) -> Error {
    Error::Protocol(format!("{stream} was not piped"))
}

fn describe(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("killed by signal {signal}");
        }
    }
    status.code().map_or_else(
        || "unknown status".into(),
        |code| format!("exit code {code}"),
    )
}
