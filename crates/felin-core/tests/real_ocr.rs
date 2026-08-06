//! Dev-only end-to-end test against the REAL ocr-cli (production sidecar).
//!
//! Not part of the normal `cargo test` run — automated tests use mock-ocr-cli.
//! This one exercises the production backend and makes real OCR API calls, so
//! it is `#[ignore]`d and gated by env vars. Run it explicitly, e.g.:
//!
//! ```bash
//! FELIN_SIDECAR=test/bin/ocr-cli \
//! FELIN_SIDECAR_CONFIG=../ocr-router/config.yaml \
//! FELIN_TEST_IMAGE=test/frey/001.png \
//! cargo test -p felin-core --test real_ocr -- --ignored
//! ```
//!
//! Without the env vars it passes trivially (a no-op), so CI / `cargo test
//! --workspace` is never affected.

use felin_core::ocr::sidecar::{run_extract, ExtractArgs, ExtractOutcome};
use felin_core::ocr::{ingest_from_manifest, read_manifest, DEFAULT_LOW_SCORE_THRESHOLD};
use std::path::PathBuf;
use tokio::sync::watch;

fn enders() -> Vec<char> {
    felin_core::config::DEFAULT_SENTENCE_ENDERS.chars().collect()
}

const CAP: u64 = 64 * 1024 * 1024;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the real ocr-cli + provider keys; run explicitly (see module doc)"]
async fn real_ocr_cli_extract_and_ingest() {
    let (Some(sidecar), Some(image)) = (
        std::env::var_os("FELIN_SIDECAR").map(PathBuf::from),
        std::env::var_os("FELIN_TEST_IMAGE").map(PathBuf::from),
    ) else {
        eprintln!("skipping real-ocr e2e: set FELIN_SIDECAR + FELIN_TEST_IMAGE (see module doc)");
        return;
    };
    let config = std::env::var_os("FELIN_SIDECAR_CONFIG").map(PathBuf::from);

    let dir = tempfile::tempdir().unwrap();
    let a = ExtractArgs {
        sidecar,
        input: image,
        config,
        out_dir: dir.path().to_path_buf(),
        manifest: dir.path().join("book.manifest.json"),
        pages: None,
        page_workers: None,
        skip_existing: false,
        extra: vec![],
        envs: vec![],
    };
    let (_tx, rx) = watch::channel(false);
    let outcome = run_extract(&a, |_| {}, rx).await.unwrap();
    assert_eq!(outcome, ExtractOutcome::AllOk, "real ocr-cli should OCR every page");

    let m = read_manifest(&a.manifest, CAP).unwrap();
    assert!(m.pages_ok >= 1, "real OCR should succeed on at least one page");
    let res =
        ingest_from_manifest(dir.path(), &m, DEFAULT_LOW_SCORE_THRESHOLD, false, &enders(), CAP)
            .unwrap();
    assert!(!res.paragraphs.is_empty(), "real OCR should yield at least one paragraph");
}
