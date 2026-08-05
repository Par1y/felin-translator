//! Integration tests for manifest reconciliation + ingestion from real on-disk
//! per-page JSON fixtures (the OCR "single entry point = manifest" path).

use felin_core::ocr::{ingest_from_manifest, read_manifest, DEFAULT_LOW_SCORE_THRESHOLD};
use felin_core::types::OcrParagraphStatus;
use std::fs;
use std::path::Path;

fn enders() -> Vec<char> {
    felin_core::config::DEFAULT_SENTENCE_ENDERS.chars().collect()
}

/// Generous read cap for tests.
const CAP: u64 = 64 * 1024 * 1024;


fn write(dir: &Path, name: &str, content: &str) {
    fs::write(dir.join(name), content).unwrap();
}

#[test]
fn ingest_reconciles_pages_merges_and_nulls_scores() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();

    // page 1: ends mid-sentence, evaluator OFF (score_present=false → NULL score)
    write(out, "b-0001.json", r#"{"page":1,"status":"ok","text":"これは途中で","score_present":false}"#);
    // page 2: continues the sentence, then a new paragraph; low score present
    write(
        out,
        "b-0002.json",
        r#"{"page":2,"status":"ok","text":"切れた文。\n\n次の段。","score":0.4,"score_present":true}"#,
    );
    // page 3: failed — a JSON exists but its text must never be ingested
    write(out, "b-0003.json", r#"{"page":3,"status":"failed","text":"garbage","error":"timeout"}"#);

    let manifest = r#"{"schema_version":1,"source":"b.pdf","type":"pdf",
        "pages_total":3,"pages_attempted":3,"pages_ok":2,"pages_failed":1,
        "pages":[
          {"page":1,"status":"ok","file":"b-0001.json"},
          {"page":2,"status":"ok","file":"b-0002.json","score":0.4},
          {"page":3,"status":"failed","file":"b-0003.json","error":"timeout"}
        ]}"#;
    write(out, "b.manifest.json", manifest);

    let m = read_manifest(&out.join("b.manifest.json"), CAP).unwrap();
    let res = ingest_from_manifest(out, &m, DEFAULT_LOW_SCORE_THRESHOLD, false, &enders(), CAP).unwrap();

    assert_eq!(res.failed_pages, vec![3], "failed page recorded for rescue");
    assert!(res.any_score_present);
    assert_eq!(res.paragraphs.len(), 2);

    // page 1 tail merged with page 2's first block; start page preserved.
    assert_eq!(res.paragraphs[0].text, "これは途中で切れた文。");
    assert_eq!(res.paragraphs[0].page_num, Some(1));
    // The merge folds in the absorbed page's quality signals: page 2 (score 0.4)
    // is low, so the merged paragraph carries that low score, not page 1's NULL.
    assert_eq!(res.paragraphs[0].page_score, Some(0.4));
    assert_eq!(res.paragraphs[0].ocr_status, OcrParagraphStatus::LowScore);

    // page 2's second block is its own paragraph, also flagged low-score.
    assert_eq!(res.paragraphs[1].text, "次の段。");
    assert_eq!(res.paragraphs[1].page_num, Some(2));
    assert_eq!(res.paragraphs[1].page_score, Some(0.4));
    assert_eq!(res.paragraphs[1].ocr_status, OcrParagraphStatus::LowScore);
}

#[test]
fn rejects_unknown_manifest_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.json");
    fs::write(
        &path,
        r#"{"schema_version":2,"source":"x","type":"pdf","pages_total":0,"pages_attempted":0,"pages_ok":0,"pages_failed":0,"pages":[]}"#,
    )
    .unwrap();
    let err = read_manifest(&path, CAP).unwrap_err();
    assert!(matches!(err, felin_core::Error::Contract { .. }), "got {err:?}");
}

#[test]
fn missing_page_json_is_a_contract_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();
    let manifest = r#"{"schema_version":1,"source":"b.pdf","type":"pdf",
        "pages_total":1,"pages_attempted":1,"pages_ok":1,"pages_failed":0,
        "pages":[{"page":1,"status":"ok","file":"does-not-exist.json"}]}"#;
    write(out, "b.manifest.json", manifest);
    let m = read_manifest(&out.join("b.manifest.json"), CAP).unwrap();
    let err = ingest_from_manifest(out, &m, DEFAULT_LOW_SCORE_THRESHOLD, false, &enders(), CAP).unwrap_err();
    assert!(matches!(err, felin_core::Error::Contract { .. }), "got {err:?}");
}

#[test]
fn rejects_path_traversal_in_page_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();
    // An absolute or escaping page file must be refused before any read.
    let manifest = r#"{"schema_version":1,"source":"b.pdf","type":"pdf",
        "pages_total":1,"pages_attempted":1,"pages_ok":1,"pages_failed":0,
        "pages":[{"page":1,"status":"ok","file":"../../../etc/passwd"}]}"#;
    write(out, "b.manifest.json", manifest);
    let m = read_manifest(&out.join("b.manifest.json"), CAP).unwrap();
    let err = ingest_from_manifest(out, &m, DEFAULT_LOW_SCORE_THRESHOLD, false, &enders(), CAP).unwrap_err();
    assert!(matches!(err, felin_core::Error::Contract { .. }), "got {err:?}");
}

#[test]
fn rejects_duplicate_page_in_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();
    write(out, "b-0001.json", r#"{"page":1,"status":"ok","text":"x。"}"#);
    let manifest = r#"{"schema_version":1,"source":"b.pdf","type":"pdf",
        "pages_total":2,"pages_attempted":2,"pages_ok":2,"pages_failed":0,
        "pages":[{"page":1,"status":"ok","file":"b-0001.json"},
                 {"page":1,"status":"ok","file":"b-0001.json"}]}"#;
    write(out, "b.manifest.json", manifest);
    let m = read_manifest(&out.join("b.manifest.json"), CAP).unwrap();
    let err = ingest_from_manifest(out, &m, DEFAULT_LOW_SCORE_THRESHOLD, false, &enders(), CAP).unwrap_err();
    assert!(matches!(err, felin_core::Error::Contract { .. }), "got {err:?}");
}
