//! Orchestration of the `ocr-cli batch` sidecar for image-directory imports.
//!
//! Unlike `extract`, `batch` emits no JSONL — progress is line-oriented on
//! stdout (`✓ name -> path`) / stderr (`✗ name: err`). It also accepts no
//! pattern/range flags, so the caller stages the *selected* images (see
//! [`crate::ocr::select`]) into a dedicated input directory first; anything not
//! staged (e.g. a PDF mixed into a real image folder) is simply never processed.
//!
//! Spawn/process-group/cancel semantics mirror [`crate::ocr::sidecar`]: own
//! process group, SIGTERM on cancel, grace period, then force-kill of the group.

use crate::error::{Error, Result};
use crate::ocr::ingest::{build_paragraphs, PageForIngest};
use crate::types::IngestedParagraph;
use command_group::AsyncCommandGroup;
#[cfg(unix)]
use command_group::{Signal, UnixChildExt};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

/// Grace period after SIGTERM before the batch process group is force-killed.
const CANCEL_GRACE: Duration = Duration::from_secs(8);

/// Everything needed to invoke `batch`. `input_dir` is the app's staging
/// directory holding exactly the selected images (by their original names).
#[derive(Debug, Clone)]
pub struct BatchArgs {
    pub sidecar: PathBuf,
    pub input_dir: PathBuf,
    pub config: Option<PathBuf>,
    pub out_dir: PathBuf,
    /// Concurrent image workers (`-w`); `None` = batch's default.
    pub workers: Option<u32>,
    pub recursive: bool,
    /// Idempotent resume: skip images whose `.txt` already exists.
    pub skip_existing: bool,
    /// Also write `<name>.json` per file (used for score/quality metadata).
    pub save_json: bool,
    /// Extra environment variables for the child (passing scenario config this
    /// way keeps concurrent invocations isolated).
    pub envs: Vec<(String, String)>,
}

impl BatchArgs {
    /// Build the argument vector (excluding the program itself). Pure, so arg
    /// construction can be unit-tested without spawning.
    pub fn to_argv(&self) -> Vec<std::ffi::OsString> {
        use std::ffi::OsString;
        let mut a: Vec<OsString> = Vec::new();
        a.push("batch".into());
        a.push(self.input_dir.clone().into_os_string());
        if let Some(c) = &self.config {
            a.push("-c".into());
            a.push(c.clone().into_os_string());
        }
        a.push("-o".into());
        a.push(self.out_dir.clone().into_os_string());
        if let Some(w) = self.workers {
            a.push("-w".into());
            a.push(w.to_string().into());
        }
        if self.recursive {
            a.push("-r".into());
        }
        if self.skip_existing {
            a.push("-s".into());
        }
        if self.save_json {
            a.push("--save-json".into());
        }
        a
    }
}

/// A single batch progress event, parsed from the sidecar's line output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchEvent {
    /// `✓ label -> path` (label = base name, or `name page N` for a PDF page).
    Done { label: String, out: String },
    /// `✗ label: err`.
    Failed { label: String, error: String },
}

/// Tally of a batch run's per-file outcomes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchOutcome {
    pub ok: usize,
    pub failed: usize,
}

/// Parse one `batch` output line into an event. Success markers arrive on
/// stdout, failure markers on stderr (and both glyphs can appear on either
/// stream); `None` means the line carried no progress (e.g. the final summary).
pub fn parse_batch_line(line: &str) -> Option<BatchEvent> {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix('✓') {
        let rest = rest.trim_start();
        if let Some(idx) = rest.find(" -> ") {
            return Some(BatchEvent::Done {
                label: rest[..idx].trim().to_string(),
                out: rest[idx + 4..].trim().to_string(),
            });
        }
        return Some(BatchEvent::Done { label: rest.to_string(), out: String::new() });
    }
    if let Some(rest) = t.strip_prefix('✗') {
        let rest = rest.trim_start();
        if let Some(idx) = rest.find(": ") {
            return Some(BatchEvent::Failed {
                label: rest[..idx].trim().to_string(),
                error: rest[idx + 2..].trim().to_string(),
            });
        }
        return Some(BatchEvent::Failed { label: rest.to_string(), error: String::new() });
    }
    None
}

/// Run `batch`, forwarding parsed per-file events to `on_progress` until the
/// sidecar exits.
///
/// `batch` exits 0 even when individual files failed (those surface as
/// [`BatchEvent::Failed`]); a non-zero exit means a startup failure (config load,
/// unreadable directory) and is reported as [`Error::OcrFatal`].
///
/// Cancellation mirrors [`crate::ocr::sidecar::run_extract`]: SIGTERM to the
/// whole process group, a grace period, then a force-kill.
pub async fn run_batch<F>(
    args: &BatchArgs,
    mut on_progress: F,
    mut cancel_rx: watch::Receiver<bool>,
) -> Result<BatchOutcome>
where
    F: FnMut(BatchEvent) + Send,
{
    let mut cmd = Command::new(&args.sidecar);
    cmd.args(args.to_argv())
        .envs(args.envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
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

    // Both streams are read in tasks and forwarded as parsed events over one
    // channel, so process-exit detection (try_wait) never depends on either
    // pipe reaching EOF.
    let (evt_tx, mut evt_rx) = tokio::sync::mpsc::unbounded_channel::<BatchEvent>();
    let reader_out = tokio::spawn(pipe_reader(stdout, evt_tx.clone()));
    let reader_err = stderr.map(|se| tokio::spawn(pipe_reader(se, evt_tx)));

    // Honor a cancellation already latched before we were called.
    let mut cancelled = *cancel_rx.borrow();
    if cancelled {
        request_stop(&mut child).await;
    }
    let mut deadline: Option<tokio::time::Instant> =
        cancelled.then(|| tokio::time::Instant::now() + CANCEL_GRACE);
    let mut watch_open = true;
    let mut readers_open = true;
    let mut poll = tokio::time::interval(Duration::from_millis(150));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let status = loop {
        let d = deadline;
        tokio::select! {
            biased;

            () = async move {
                match d {
                    Some(t) => tokio::time::sleep_until(t).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                tracing::warn!("grace elapsed; force-killing batch process group");
                let _ = child.kill().await;
                deadline = None;
            }

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

            ev = evt_rx.recv(), if readers_open => match ev {
                Some(ev) => on_progress(ev),
                None => readers_open = false,
            },

            _ = poll.tick() => match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => return Err(Error::sidecar(format!("failed to poll sidecar: {e}"))),
            },
        }
    };

    // Stop both readers, then drain the channel. The stderr reader is awaited
    // (not just aborted) so its per-file failure events are never dropped by a
    // premature `try_recv` drain — the caller's ok/failed counts depend on them.
    reader_out.abort();
    if let Some(t) = reader_err {
        let _ = t.await;
    }
    while let Ok(ev) = evt_rx.try_recv() {
        on_progress(ev);
    }

    if cancelled {
        return Ok(BatchOutcome::default());
    }
    match status.code() {
        Some(0) => {
            // Per-file failures were already delivered as events; the caller
            // counts them. Exit 0 = the run itself completed.
            Ok(BatchOutcome::default())
        }
        Some(code) => Err(Error::OcrFatal {
            exit_code: code,
            message: format!("batch exited with code {code} (startup failure: bad config or unreadable directory)"),
        }),
        None => Err(Error::OcrFatal {
            exit_code: -1,
            message: "batch terminated by a signal without a cancellation request".into(),
        }),
    }
}

/// Task body: drain a stream line by line, forwarding parsed batch events.
async fn pipe_reader<R: tokio::io::AsyncRead + Unpin>(
    stream: R,
    tx: tokio::sync::mpsc::UnboundedSender<BatchEvent>,
) {
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(ev) = parse_batch_line(&line) {
            if tx.send(ev).is_err() {
                break;
            }
        }
    }
}

/// Ask the batch process group to stop. Unix: SIGTERM to the whole group; the
/// sidecar drains in-flight files and exits. Elsewhere: go straight to a kill.
async fn request_stop(child: &mut command_group::AsyncGroupChild) {
    #[cfg(unix)]
    {
        if let Err(e) = child.signal(Signal::SIGTERM) {
            tracing::warn!(error = %e, "SIGTERM failed; force-killing batch group");
            let _ = child.kill().await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill().await;
    }
}

/// Strip the trailing `[Score: 0.92]` marker that `batch` appends to a txt when
/// the evaluator ran — it is diagnostics, not OCR text, and would otherwise
/// become a spurious paragraph.
pub fn strip_batch_score_marker(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    if let Some(last) = lines.last() {
        let t = last.trim();
        if t.starts_with("[Score:") && t.ends_with(']') {
            lines.pop();
        }
    }
    // The marker sat on its own line; dropping it can leave a trailing blank.
    lines.join("\n").trim_end().to_string()
}

/// The `<name>.json` metadata `batch` writes under `--save-json` (a slice of the
/// sidecar's `OCRResult`; unknown fields are ignored).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BatchJson {
    quality_warning: bool,
    #[serde(default)]
    best_score: Option<f64>,
    fallback: bool,
    evaluation: Option<BatchEvaluation>,
}

#[derive(Debug, Deserialize)]
struct BatchEvaluation {
    score: f64,
}

/// Assemble paragraphs from a batch run's per-image txt outputs.
///
/// Each selected image `files[i]` produced `<out_dir>/<stem>.txt` (stem = the
/// image's file name minus extension); when `--save-json` was on, a sibling
/// `<stem>.json` carries the score/quality metadata. Each txt is treated as one
/// page (numbered by the caller's order) and fed through the same
/// `split_blocks` / cross-page merge path as `extract` output. `source_file` on
/// the resulting paragraphs is the *original* image path, not the staged copy.
///
/// Files whose txt is missing (the sidecar reported a failure) are skipped; the
/// caller decides how to surface that from the batch events.
pub fn ingest_batch_txts(
    files: &[PathBuf],
    out_dir: &Path,
    low_score_threshold: f64,
    sentence_enders: &[char],
) -> Result<Vec<IngestedParagraph>> {
    let mut pages: Vec<PageForIngest> = Vec::new();
    for (idx, file) in files.iter().enumerate() {
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let txt_path = out_dir.join(format!("{stem}.txt"));
        let Ok(text) = std::fs::read_to_string(&txt_path) else { continue };

        let mut meta = BatchJson::default();
        let json_path = out_dir.join(format!("{stem}.json"));
        if let Ok(data) = std::fs::read(&json_path) {
            if let Ok(parsed) = serde_json::from_slice::<BatchJson>(&data) {
                meta = parsed;
            }
        }
        let score = meta
            .evaluation
            .as_ref()
            .map(|e| e.score)
            .or_else(|| meta.best_score.filter(|s| *s > 0.0));
        let best_score = meta.best_score.or(score);

        pages.push(PageForIngest {
            // batch has no page numbers; the natural order index is the page.
            page: (idx + 1) as i64,
            text: strip_batch_score_marker(&text),
            score,
            quality_warning: meta.quality_warning,
            blank: false,
            best_score,
            fallback: meta.fallback,
            source_file: file.to_string_lossy().into_owned(),
            recovered: false,
        });
    }
    Ok(build_paragraphs(&pages, low_score_threshold, sentence_enders))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_image_success_and_failure_lines() {
        assert_eq!(
            parse_batch_line("✓ 002.png -> /tmp/out/002.txt"),
            Some(BatchEvent::Done { label: "002.png".into(), out: "/tmp/out/002.txt".into() })
        );
        assert_eq!(
            parse_batch_line("✗ 155a.jpg: recognition failed: provider down"),
            Some(BatchEvent::Failed { label: "155a.jpg".into(), error: "recognition failed: provider down".into() })
        );
        assert_eq!(parse_batch_line("Completed: 2 success, 1 failed, 3 total"), None);
        assert_eq!(parse_batch_line("No files found"), None);
        assert_eq!(parse_batch_line(""), None);
    }

    #[test]
    fn parses_pdf_page_lines_on_either_glyph_stream() {
        // PDF success is `✓ name page N -> path`, failure `✗ name page N: err`.
        assert_eq!(
            parse_batch_line("✓ book.pdf page 3 -> /tmp/out/book_003.txt"),
            Some(BatchEvent::Done { label: "book.pdf page 3".into(), out: "/tmp/out/book_003.txt".into() })
        );
        assert_eq!(
            parse_batch_line("✗ book.pdf page 3: timeout"),
            Some(BatchEvent::Failed { label: "book.pdf page 3".into(), error: "timeout".into() })
        );
    }

    #[test]
    fn strips_evaluator_score_marker() {
        assert_eq!(strip_batch_score_marker("本文。\n\n[Score: 0.92]"), "本文。");
        // no marker → unchanged
        assert_eq!(strip_batch_score_marker("本文。"), "本文。");
        // interior [Score:...] line is not stripped
        assert_eq!(strip_batch_score_marker("本文。[Score: 0.5]\n次段。"), "本文。[Score: 0.5]\n次段。");
    }

    #[test]
    fn argv_is_deterministic() {
        let args = BatchArgs {
            sidecar: PathBuf::from("/bin/ocr-cli"),
            input_dir: PathBuf::from("/tmp/inputs"),
            config: Some(PathBuf::from("/tmp/config.yaml")),
            out_dir: PathBuf::from("/tmp/out"),
            workers: Some(4),
            recursive: false,
            skip_existing: false,
            save_json: true,
            envs: vec![],
        };
        let a: Vec<String> = args.to_argv().into_iter().map(|s| s.into_string().unwrap()).collect();
        assert_eq!(
            a,
            vec!["batch", "/tmp/inputs", "-c", "/tmp/config.yaml", "-o", "/tmp/out", "-w", "4", "--save-json"]
        );
    }

    #[test]
    fn ingest_treats_each_txt_as_a_page_and_reads_json_metadata() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        // Two selected images → two txt outputs (+ one json with evaluator data).
        let files = vec![dir.path().join("001.png"), dir.path().join("002.png")];
        fs::write(out.join("001.txt"), "第一页の文。\n\n[Score: 0.91]").unwrap();
        fs::write(
            out.join("001.json"),
            r#"{"text":"第一页の文。","quality_warning":false,"best_score":0.91,"fallback":false,"evaluation":{"score":0.91,"reason":"ok","pass":true}}"#,
        )
        .unwrap();
        fs::write(out.join("002.txt"), "第二页の文。").unwrap();

        let paras = ingest_batch_txts(&files, &out, 0.6, &['。', '！', '？']).unwrap();
        assert_eq!(paras.len(), 2);
        assert_eq!(paras[0].text, "第一页の文。");
        assert_eq!(paras[0].page_num, Some(1));
        assert_eq!(paras[0].page_score, Some(0.91));
        assert!(paras[0].source_file.ends_with("001.png"));
        assert_eq!(paras[1].page_num, Some(2));
        assert!(paras[1].source_file.ends_with("002.png"));
    }

    #[test]
    fn ingest_skips_txts_the_sidecar_failed_to_write() {
        use std::fs;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        let files = vec![dir.path().join("001.png"), dir.path().join("002.png")];
        // 002.txt never written (failed image).
        fs::write(out.join("001.txt"), "第一页の文。").unwrap();
        let paras = ingest_batch_txts(&files, &out, 0.6, &['。']).unwrap();
        assert_eq!(paras.len(), 1);
        assert!(paras[0].source_file.ends_with("001.png"));
    }
}
