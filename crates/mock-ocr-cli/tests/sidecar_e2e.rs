//! End-to-end tests that drive `felin-core`'s sidecar spawner against the real
//! `mock-ocr-cli` binary (path provided by cargo as `CARGO_BIN_EXE_mock-ocr-cli`).
//! This exercises the full path: spawn → JSONL progress → manifest reconciliation
//! → exit-code mapping → graceful cancellation.

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
