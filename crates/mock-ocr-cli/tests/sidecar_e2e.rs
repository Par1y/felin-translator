//! End-to-end tests that drive `felin-core`'s sidecar spawner against the real
//! `mock-ocr-cli` binary (path provided by cargo as `CARGO_BIN_EXE_mock-ocr-cli`).
//! This exercises the full path: spawn → JSONL progress → manifest reconciliation
//! → exit-code mapping → graceful cancellation.

use felin_core::ocr::batch::{ingest_batch_txts, run_batch, BatchArgs, BatchEvent};
use felin_core::ocr::sidecar::{run_extract, ExtractArgs, ExtractOutcome};
use felin_core::ocr::{ingest_from_manifest, read_manifest, ProgressEvent, DEFAULT_LOW_SCORE_THRESHOLD};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

fn enders() -> Vec<char> {
    felin_core::config::DEFAULT_SENTENCE_ENDERS.chars().collect()
}

const CAP: u64 = 64 * 1024 * 1024;


fn mock_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mock-ocr-cli"))
}

fn args(out: &Path, input: &str, envs: &[(&str, &str)]) -> ExtractArgs {
    ExtractArgs {
        sidecar: mock_bin(),
        input: PathBuf::from(input),
        config: None,
        out_dir: out.to_path_buf(),
        manifest: out.join("book.manifest.json"),
        pages: None,
        page_workers: None,
        skip_existing: false,
        extra: vec![],
        envs: envs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    }
}

fn batch_args(dir: &Path, out: &Path, envs: &[(&str, &str)]) -> BatchArgs {
    BatchArgs {
        sidecar: mock_bin(),
        input_dir: dir.to_path_buf(),
        config: None,
        out_dir: out.to_path_buf(),
        workers: None,
        recursive: false,
        skip_existing: false,
        save_json: true,
        envs: envs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    }
}

/// Create a set of (mostly image) files so the mock can list them.
fn touch(dir: &Path, names: &[&str]) {
    for n in names {
        std::fs::write(dir.join(n), b"x").unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_ok_emits_progress_and_full_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let a = args(dir.path(), "book.pdf", &[("MOCK_OCR_PAGES", "4")]);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let (_tx, rx) = watch::channel(false);

    let outcome = run_extract(&a, move |e| sink.lock().unwrap().push(e), rx).await.unwrap();
    assert_eq!(outcome, ExtractOutcome::AllOk);

    let evs = events.lock().unwrap();
    assert!(matches!(evs.first(), Some(ProgressEvent::Start { pages_total: 4, .. })));
    assert!(matches!(evs.last(), Some(ProgressEvent::Done { pages_ok: 4, pages_failed: 0, .. })));

    let m = read_manifest(&a.manifest, CAP).unwrap();
    assert_eq!((m.pages_ok, m.pages_failed), (4, 0));
    let res = ingest_from_manifest(dir.path(), &m, DEFAULT_LOW_SCORE_THRESHOLD, false, &enders(), CAP).unwrap();
    assert_eq!(res.paragraphs.len(), 4, "each mock page is one complete sentence");
    assert!(res.failed_pages.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_page_maps_to_partial_and_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let a = args(dir.path(), "book.pdf", &[("MOCK_OCR_PAGES", "3"), ("MOCK_OCR_FAIL_PAGES", "2")]);
    let (_tx, rx) = watch::channel(false);

    let outcome = run_extract(&a, |_| {}, rx).await.unwrap();
    assert_eq!(outcome, ExtractOutcome::Partial);

    let m = read_manifest(&a.manifest, CAP).unwrap();
    assert_eq!(m.pages_failed, 1);
    let res = ingest_from_manifest(dir.path(), &m, DEFAULT_LOW_SCORE_THRESHOLD, false, &enders(), CAP).unwrap();
    assert_eq!(res.failed_pages, vec![2]);
    assert_eq!(res.paragraphs.len(), 2, "pages 1 and 3 only");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fatal_startup_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let a = args(dir.path(), "book.pdf", &[("MOCK_OCR_FATAL", "1")]);
    let (_tx, rx) = watch::channel(false);

    let err = run_extract(&a, |_| {}, rx).await.unwrap_err();
    assert!(matches!(err, felin_core::Error::OcrFatal { exit_code: 20, .. }), "got {err:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_flushes_a_cancelled_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let manifest_path = dir.path().join("book.manifest.json");
    let a = args(dir.path(), "book.pdf", &[("MOCK_OCR_PAGES", "50"), ("MOCK_OCR_PAGE_DELAY_MS", "80")]);

    let (tx, rx) = watch::channel(false);
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = tokio::spawn(async move { run_extract(&a, move |e| { let _ = ev_tx.send(e); }, rx).await });

    // Cancel as soon as the sidecar reports progress.
    let _first = ev_rx.recv().await.expect("expected at least one progress event");
    tx.send(true).unwrap();

    let outcome = handle.await.unwrap().unwrap();
    assert_eq!(outcome, ExtractOutcome::Cancelled);

    let m = read_manifest(&manifest_path, CAP).unwrap();
    assert_eq!(m.status.as_deref(), Some("cancelled"));
    assert!(m.pages.len() < 50, "should have been cancelled before all pages");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn skip_existing_resumes_only_missing_pages() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = args(dir.path(), "book.pdf", &[("MOCK_OCR_PAGES", "5")]);

    let (_tx, rx) = watch::channel(false);
    run_extract(&a, |_| {}, rx).await.unwrap();
    assert!(dir.path().join("book-0003.json").exists());

    // Simulate a crash that lost page 3, then resume.
    std::fs::remove_file(dir.path().join("book-0003.json")).unwrap();
    a.skip_existing = true;
    let (_tx2, rx2) = watch::channel(false);
    let outcome = run_extract(&a, |_| {}, rx2).await.unwrap();

    assert_eq!(outcome, ExtractOutcome::AllOk);
    assert!(dir.path().join("book-0003.json").exists(), "missing page was re-OCR'd");
    let m = read_manifest(&a.manifest, CAP).unwrap();
    assert_eq!(m.pages_ok, 5);
}

// ---------------------------------------------------------------------------
// `batch` mode (image-directory flow)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_emits_done_for_each_image_and_skips_non_images() {
    let dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    // 155a.jpg sorts after 001/002; the mixed-in PDF and md are non-expected.
    touch(dir.path(), &["001.png", "002.jpg", "155a.jpg", "notepage.pdf", "readme.md"]);

    let a = batch_args(
        dir.path(),
        out.path(),
        &[("MOCK_OCR_EVALUATOR", "1"), ("MOCK_OCR_TEXT", "これはテストページ{page}の本文です。")],
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let (_tx, rx) = watch::channel(false);

    let outcome = run_batch(&a, move |e| sink.lock().unwrap().push(e), rx).await.unwrap();
    assert!(outcome.ok == 0, "batch counts per-file outcomes via events, not the return");

    let evs = events.lock().unwrap();
    let mut labels: Vec<&str> = evs.iter().filter_map(|e| match e {
        BatchEvent::Done { label, .. } => Some(label.as_str()),
        BatchEvent::Failed { .. } => None,
    }).collect();
    labels.sort();
    assert_eq!(labels, vec!["001.png", "002.jpg", "155a.jpg"], "PDF/md are skipped");
    assert!(evs.iter().all(|e| matches!(e, BatchEvent::Done { .. })), "no failures");

    for stem in ["001", "002", "155a"] {
        assert!(out.path().join(format!("{stem}.txt")).exists(), "{stem}.txt written");
        assert!(out.path().join(format!("{stem}.json")).exists(), "{stem}.json written (--save-json + evaluator)");
    }
    assert!(!out.path().join("notepage.txt").exists(), "PDF never processed");
    assert!(!out.path().join("readme.txt").exists(), "md never processed");

    // The evaluator appended `[Score: …]`; ingest strips it before splitting.
    let files: Vec<PathBuf> = ["001.png", "002.jpg", "155a.jpg"].iter().map(|n| dir.path().join(n)).collect();
    let res = ingest_batch_txts(&files, out.path(), DEFAULT_LOW_SCORE_THRESHOLD, &enders()).unwrap();
    assert_eq!(res.len(), 3, "one paragraph per image");
    assert!(res.iter().all(|p| !p.text.contains("[Score:")), "score marker stripped");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_failure_is_reported_as_event_but_run_exits_ok() {
    let dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    touch(dir.path(), &["001.png", "bad.png"]);

    let a = batch_args(dir.path(), out.path(), &[("MOCK_OCR_BATCH_FAIL", "bad")]);
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    let (_tx, rx) = watch::channel(false);

    // `batch` exits 0 even with per-file failures; only a startup failure is fatal.
    run_batch(&a, move |e| sink.lock().unwrap().push(e), rx).await.unwrap();

    let evs = events.lock().unwrap();
    assert!(evs.iter().any(|e| matches!(e, BatchEvent::Failed { label, .. } if label == "bad.png")));
    assert!(evs.iter().any(|e| matches!(e, BatchEvent::Done { label, .. } if label == "001.png")));
    assert!(out.path().join("001.txt").exists());
    assert!(!out.path().join("bad.txt").exists(), "failed file produces no txt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_cancellation_stops_partway() {
    let dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    touch(dir.path(), &["001.png", "002.png", "003.png", "004.png", "005.png"]);

    let a = batch_args(dir.path(), out.path(), &[("MOCK_OCR_BATCH_DELAY_MS", "80")]);
    let (tx, rx) = watch::channel(false);
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = tokio::spawn(async move { run_batch(&a, move |e| { let _ = ev_tx.send(e); }, rx).await });

    // Cancel as soon as the first file reports.
    let _first = ev_rx.recv().await.expect("expected at least one batch event");
    tx.send(true).unwrap();

    let res = handle.await.unwrap().unwrap();
    assert!(res.ok == 0 && res.failed == 0, "cancelled runs return the default outcome");

    // The mock drains to the SIGTERM and stops; not all txts are produced.
    let produced: Vec<_> = ["001", "002", "003", "004", "005"]
        .iter()
        .filter(|s| out.path().join(format!("{s}.txt")).exists())
        .collect();
    assert!(!produced.is_empty(), "at least one file completed before cancel");
    assert!(produced.len() < 5, "cancelled before all files: got {produced:?}");
}
