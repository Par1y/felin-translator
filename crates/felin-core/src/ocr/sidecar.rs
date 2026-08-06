//! Async orchestration of the `ocr-cli extract` sidecar: spawn in its own
//! process group, stream `--progress json` events, and support graceful
//! cancellation that also reaps grandchildren (pdftoppm/mutool).
//!
//! Tauri-agnostic: the caller passes the resolved sidecar path and a progress
//! callback, and drives cancellation via a `watch` channel.

use crate::error::{Error, Result};
use crate::ocr::contract::ProgressEvent;
use command_group::AsyncCommandGroup;
#[cfg(unix)]
use command_group::{Signal, UnixChildExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

/// Grace period after SIGTERM before the process group is force-killed.
const CANCEL_GRACE: Duration = Duration::from_secs(8);

/// Everything needed to invoke `extract`. The `sidecar` path is resolved by the
/// app shell (next to the main executable) and passed in.
#[derive(Debug, Clone)]
pub struct ExtractArgs {
    pub sidecar: PathBuf,
    pub input: PathBuf,
    pub config: Option<PathBuf>,
    pub out_dir: PathBuf,
    pub manifest: PathBuf,
    /// Arbitrary page set for PDFs, e.g. `"3,7,10-12"`. `None` = all pages.
    pub pages: Option<String>,
    pub page_workers: Option<u32>,
    /// Idempotent resume: skip pages already `status:"ok"`.
    pub skip_existing: bool,
    /// Extra passthrough flags (escape hatch, e.g. `--overlap-render`).
    pub extra: Vec<String>,
    /// Extra environment variables to set on the child process. Passing scenario
    /// config this way (rather than via the parent's process env) keeps
    /// concurrent invocations isolated.
    pub envs: Vec<(String, String)>,
}

impl ExtractArgs {
    /// Build the argument vector (excluding the program itself). Pure, so arg
    /// construction can be unit-tested without spawning.
    pub fn to_argv(&self) -> Vec<std::ffi::OsString> {
        use std::ffi::OsString;
        let mut a: Vec<OsString> = Vec::new();
        a.push("extract".into());
        a.push(self.input.clone().into_os_string());
        if let Some(c) = &self.config {
            a.push("-c".into());
            a.push(c.clone().into_os_string());
        }
        a.push("--out".into());
        a.push(self.out_dir.clone().into_os_string());
        a.push("--manifest".into());
        a.push(self.manifest.clone().into_os_string());
        if let Some(p) = &self.pages {
            a.push("--pages".into());
            a.push(p.into());
        }
        if let Some(w) = self.page_workers {
            a.push("--page-workers".into());
            a.push(w.to_string().into());
        }
        if self.skip_existing {
            a.push("--skip-existing".into());
        }
        a.push("--progress".into());
        a.push("json".into());
        for e in &self.extra {
            a.push(e.into());
        }
        a
    }
}

/// How an extract run finished — drives whether the app treats output as fully
/// usable, partially usable (rescue failed pages), or aborted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractOutcome {
    /// Exit 0 — every page OK.
    AllOk,
    /// Exit 10 — completed with some failed pages (see manifest).
    Partial,
    /// Exit 130 / killed — cancelled.
    Cancelled,
}

/// Run `extract`, forwarding each parsed progress event to `on_progress` until
/// the sidecar exits.
///
/// Cancellation: set `cancel_rx` to `true` (or drop its sender) → SIGTERM to the
/// whole process group (grandchildren included), a grace period to flush a
/// `status:"cancelled"` manifest, then a force-kill. Exit codes map to
/// [`ExtractOutcome`] / [`Error::OcrFatal`] per contract §4.
pub async fn run_extract<F>(
    args: &ExtractArgs,
    mut on_progress: F,
    mut cancel_rx: watch::Receiver<bool>,
) -> Result<ExtractOutcome>
where
    F: FnMut(ProgressEvent) + Send,
{
    let mut cmd = Command::new(&args.sidecar);
    cmd.args(args.to_argv())
        .envs(args.envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd
        .group_spawn()
        .map_err(|e| Error::sidecar(format!("failed to spawn {}: {e}", args.sidecar.display())))?;

    let stdout = child
        .inner()
        .stdout
        .take()
        .ok_or_else(|| Error::sidecar("sidecar stdout was not captured"))?;
    let stderr = child.inner().stderr.take();

    // Read stdout in its own task, forwarding parsed events over a channel, so
    // that process-exit detection (try_wait, below) never depends on stdout
    // reaching EOF — a lingering grandchild holding the pipe open can't hang us.
    let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<ProgressEvent>();
    let reader = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            match serde_json::from_str::<ProgressEvent>(t) {
                Ok(ev) => {
                    if evt_tx.send(ev).is_err() {
                        break;
                    }
                }
                Err(e) => tracing::warn!(line = t, error = %e, "unparseable progress line"),
            }
        }
    });

    // Drain stderr at the byte level (UTF-8-agnostic, so non-UTF-8 output from C
    // helpers cannot abort the drain and dead-lock the child on a full pipe),
    // while keeping a bounded tail so fatal-startup errors (exit 20) can surface
    // ocr-cli's real reason instead of the generic contract message.
    let stderr_buf = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let stderr_task = stderr.map(|se| {
        let buf = Arc::clone(&stderr_buf);
        tokio::spawn(async move {
            let mut r = BufReader::new(se);
            let mut chunk = Vec::new();
            const CAP: usize = 16 * 1024;
            loop {
                chunk.clear();
                match r.read_until(b'\n', &mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let mut b = buf.lock().unwrap_or_else(|p| p.into_inner());
                        b.extend_from_slice(&chunk);
                        if b.len() > CAP {
                            let start = b.len() - CAP;
                            b.drain(..start);
                        }
                    }
                }
            }
        })
    });

    // Honor a cancellation already latched before we were called.
    let mut cancelled = *cancel_rx.borrow();
    if cancelled {
        request_stop(&mut child).await;
    }
    let mut deadline: Option<tokio::time::Instant> =
        cancelled.then(|| tokio::time::Instant::now() + CANCEL_GRACE);
    let mut watch_open = true;
    let mut reader_open = true;
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(150));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let status = loop {
        // Copy the deadline so the timer future owns it (no borrow of `deadline`
        // is held across the select; the cancel branch mutates it).
        let d = deadline;
        tokio::select! {
            biased;

            // Grace elapsed after SIGTERM → force-kill the whole group.
            () = async move {
                match d {
                    Some(t) => tokio::time::sleep_until(t).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                tracing::warn!("grace elapsed; force-killing sidecar process group");
                let _ = child.kill().await;
                deadline = None;
            }

            // Cancellation request. A dropped sender means no further cancellation
            // is possible — stop watching rather than treat it as a spurious cancel.
            r = cancel_rx.changed(), if watch_open && !cancelled => match r {
                Ok(()) => {
                    if *cancel_rx.borrow() {
                        cancelled = true;
                        request_stop(&mut child).await;
                        deadline = Some(tokio::time::Instant::now() + CANCEL_GRACE);
                    }
                }
                Err(_) => watch_open = false,
            },

            // Progress events forwarded by the reader task.
            ev = evt_rx.recv(), if reader_open => match ev {
                Some(ev) => on_progress(ev),
                None => reader_open = false,
            },

            // Authoritative exit detection, independent of the stdout pipe.
            _ = poll.tick() => match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => return Err(Error::sidecar(format!("failed to poll sidecar: {e}"))),
            },
        }
    };

    // Deliver any events the reader already buffered, then stop the helpers.
    while let Ok(ev) = evt_rx.try_recv() {
        on_progress(ev);
    }
    reader.abort();
    if let Some(t) = stderr_task {
        let _ = t.await;
    }
    // Pull the captured stderr tail so fatal-startup errors surface the real
    // cause (e.g. ocr-cli's "failed to load config: open config.yaml: ...").
    let stderr_tail = {
        let b = stderr_buf.lock().unwrap_or_else(|p| p.into_inner());
        String::from_utf8_lossy(&b).into_owned()
    };
    let detail = stderr_suffix(&stderr_tail);

    if cancelled {
        return Ok(ExtractOutcome::Cancelled);
    }
    // We never requested a stop, so a signal death — or a self-reported 130 — is
    // a real failure, not a user cancellation.
    match status.code() {
        Some(0) => Ok(ExtractOutcome::AllOk),
        Some(10) => Ok(ExtractOutcome::Partial),
        Some(20) => Err(Error::OcrFatal {
            exit_code: 20,
            message: format!("fatal startup error: config invalid / renderer missing / no provider{detail}"),
        }),
        Some(code) => Err(Error::OcrFatal {
            exit_code: code,
            message: format!("sidecar exited with unexpected code {code}{detail}"),
        }),
        None => Err(Error::OcrFatal {
            exit_code: -1,
            message: "sidecar terminated by a signal without a cancellation request".into(),
        }),
    }
}

/// Append the captured sidecar stderr tail (trimmed, line-prefixed) so the error
/// message carries ocr-cli's own diagnostic rather than only the contract code.
fn stderr_suffix(tail: &str) -> String {
    let t = tail.trim();
    if t.is_empty() {
        String::new()
    } else {
        format!("\nsidecar stderr:\n{t}")
    }
}

/// Ask the sidecar to stop. Unix: SIGTERM to the whole group (it flushes a
/// `status:"cancelled"` manifest and exits 130, per contract §5). Elsewhere: go
/// straight to a group kill (no SIGTERM equivalent).
async fn request_stop(child: &mut command_group::AsyncGroupChild) {
    #[cfg(unix)]
    {
        if let Err(e) = child.signal(Signal::SIGTERM) {
            tracing::warn!(error = %e, "SIGTERM failed; force-killing");
            let _ = child.kill().await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill().await;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::error::Error;
    use tokio::sync::watch;

    /// A fake sidecar that writes a diagnostic to stderr and exits 20. The
    /// contract's generic "fatal startup error" message hides the real cause
    /// (e.g. ocr-cli's "failed to load config: ..."); the captured stderr tail
    /// must surface in the OcrFatal message.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fatal_startup_surfaces_sidecar_stderr() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-sidecar.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\necho 'failed to load config: open config.yaml: no such file or directory' >&2\nexit 20\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let args = ExtractArgs {
            sidecar: script,
            input: dir.path().join("in.png"),
            config: None,
            out_dir: dir.path().join("out"),
            manifest: dir.path().join("out/m.manifest.json"),
            pages: None,
            page_workers: None,
            skip_existing: false,
            extra: vec![],
            envs: vec![],
        };
        let (_tx, rx) = watch::channel(false);
        let err = run_extract(&args, |_| {}, rx).await.unwrap_err();
        match err {
            Error::OcrFatal { exit_code, message } => {
                assert_eq!(exit_code, 20);
                assert!(
                    message.contains("failed to load config"),
                    "message should carry the sidecar's own stderr, got: {message}"
                );
            }
            other => panic!("expected OcrFatal, got {other:?}"),
        }
    }
}

