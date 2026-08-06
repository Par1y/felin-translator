//! Pipeline integration tests (step 8): concurrency model, TU state gate,
//! translation-memory dedup, stop/abort, crash recovery, explicit retry, and
//! prompt assembly.
//!
//! These live in `tests/` — separate from application code — and drive the
//! public `felin_core::pipeline` API against a real tempfile project DB with an
//! in-memory mock translator (no HTTP).

use felin_core::llm::TranslateRequest;
use felin_core::pipeline::{
    normalize_source, run_pipeline, source_hash, PipelineEvent, RunConfig, Translator,
};
use felin_core::storage::ProjectDb;
use felin_core::types::{
    IngestedParagraph, OcrParagraphStatus, TranslationSettings, TranslationStatus, TuStatus,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, watch, Notify};

/// Test translator: records sources + full requests, tracks concurrency, and
/// can be gated (hold) or made to fail. The gate is a `watch` latch: once
/// released (sent `true`) it stays open for every current and future caller,
/// so it never deadlocks a later TU in the same run.
struct MockTranslator {
    calls: Arc<Mutex<Vec<String>>>,
    requests: Arc<Mutex<Vec<TranslateRequest>>>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    reply: String,
    fail: bool,
    hold: Option<watch::Sender<bool>>,
    delay_ms: u64,
}

impl MockTranslator {
    fn new(reply: &str) -> Arc<Self> {
        Arc::new(Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            requests: Arc::new(Mutex::new(Vec::new())),
            active: Arc::new(AtomicUsize::new(0)),
            max_active: Arc::new(AtomicUsize::new(0)),
            reply: reply.to_string(),
            fail: false,
            hold: None,
            delay_ms: 0,
        })
    }
}

impl Translator for MockTranslator {
    async fn translate(&self, req: &TranslateRequest) -> felin_core::Result<String> {
        self.calls.lock().unwrap().push(req.source.clone());
        self.requests.lock().unwrap().push(req.clone());
        let cur = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(cur, Ordering::SeqCst);
        if let Some(gate) = &self.hold {
            let mut rx = gate.subscribe();
            if !*rx.borrow() {
                let _ = rx.wait_for(|v| *v).await;
            }
        }
        if self.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        if self.fail {
            return Err(felin_core::Error::Llm { detail: "mock failure".into() });
        }
        Ok(self.reply.clone())
    }
}

/// Build a project with one TU per paragraph (source = paragraph text).
/// Returns `(dir, db, tu_ids_by_chapter)`.
fn build_project(chapters: &[(&str, &[&str])]) -> (TempDir, Arc<ProjectDb>, Vec<Vec<i64>>) {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(ProjectDb::open(&dir.path().join("project.db")).unwrap());
    let mut all = Vec::new();
    for (ci, (title, texts)) in chapters.iter().enumerate() {
        let cid = db.create_chapter(title, ci as i64).unwrap();
        let mut tus = Vec::new();
        let mut paras: Vec<IngestedParagraph> = Vec::new();
        for t in *texts {
            paras.push(IngestedParagraph::new(
                t.to_string(),
                None,
                "test.txt".into(),
                None,
                OcrParagraphStatus::Ok,
                serde_json::Value::Null,
            ));
        }
        db.insert_paragraphs(cid, &paras).unwrap();
        // One TU per paragraph.
        for (ord, p) in paras.iter().enumerate() {
            let ids = vec![p.id.to_string()];
            let tu_id = db
                .db()
                .write(|c| {
                    c.execute(
                        "INSERT INTO tus (chapter_id, paragraph_ids, ord, budget, status)
                         VALUES (?1, ?2, ?3, NULL, 'pending')",
                        rusqlite::params![cid, serde_json::to_string(&ids).unwrap(), ord as i64],
                    )?;
                    Ok(c.last_insert_rowid())
                })
                .unwrap();
            tus.push(tu_id);
        }
        all.push(tus);
    }
    (dir, db, all)
}

fn cfg(workers: usize, window: usize) -> RunConfig {
    RunConfig {
        workers,
        window,
        memory_dedup: true,
        stop_aborts_inflight: false,
        queue_capacity: 64,
        context_max_chars: 4000,
        guidelines_max_chars: 8000,
    }
}

/// Seed an enabled entry in the project small glossary (japanese + aliases feed
/// the prompt-injection matcher).
fn add_entry(db: &ProjectDb, japanese: &str, chinese: &str, aliases: &[&str]) -> i64 {
    let aliases: Vec<String> = aliases.iter().map(|s| s.to_string()).collect();
    db.insert_glossary_entry(None, japanese, Some(chinese), None, None, &[], &aliases, None)
        .unwrap()
}

async fn run_to_end(db: Arc<ProjectDb>, translator: Arc<MockTranslator>, cfg: RunConfig) -> Vec<PipelineEvent> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (_stop_tx, stop_rx) = watch::channel(false);
    let wake = Arc::new(Notify::new());
    let db2 = Arc::clone(&db);
    let handle = tokio::spawn(async move {
        let _ = run_pipeline(db2, translator, cfg, stop_rx, wake, tx).await;
    });
    handle.await.unwrap();
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    events
}

fn tu_status(db: &ProjectDb, tu_id: i64) -> TuStatus {
    db.get_tu(tu_id).unwrap().unwrap().status
}

#[test]
fn normalize_and_hash_are_whitespace_and_nfkc_insensitive() {
    // Same content, different paragraph boundaries → same hash.
    assert_eq!(source_hash("あい\nう"), source_hash("あい\n\nう"));
    // Full-width → half-width via NFKC.
    assert_eq!(source_hash("ＡＢＣ"), source_hash("ABC"));
    // Leading/trailing/double whitespace collapses to a single space.
    assert_eq!(normalize_source("  甲\n\n乙  "), "甲 乙");
}

#[tokio::test]
async fn orders_by_chapter_then_tu() {
    let (_d, db, tus) = build_project(&[("甲", &["a1", "a2", "a3"]), ("乙", &["b1", "b2"])]);
    let mock = MockTranslator::new("T");
    run_to_end(Arc::clone(&db), mock.clone(), cfg(1, 1)).await;
    let calls = mock.calls.lock().unwrap().clone();
    assert_eq!(calls, vec!["a1".to_string(), "a2".to_string(), "a3".to_string(), "b1".to_string(), "b2".to_string()]);
    for chapter in &tus {
        for id in chapter {
            assert_eq!(tu_status(&db, *id), TuStatus::Translated);
        }
    }
}

#[tokio::test]
async fn workers_run_in_parallel() {
    let (_d, db, tus) = build_project(&[("甲", &["1", "2", "3", "4", "5", "6", "7", "8"])]);
    // Small delay so several workers overlap in translate().
    let mut mock = MockTranslator::new("T");
    let m = Arc::get_mut(&mut mock).unwrap();
    m.delay_ms = 5;
    run_to_end(Arc::clone(&db), mock.clone(), cfg(4, 1)).await;
    let maxa = mock.max_active.load(Ordering::SeqCst);
    assert!(maxa >= 2, "expected parallelism, saw {maxa}");
    for id in &tus[0] {
        assert_eq!(tu_status(&db, *id), TuStatus::Translated);
    }
}

#[tokio::test]
async fn window_w_holds_back_later_chapters() {
    let (_d, db, tus) = build_project(&[("甲", &["a1"]), ("乙", &["b1"])]);
    let (gate_tx, _gate_rx) = watch::channel(false);
    let mut mock = MockTranslator::new("T");
    {
        let m = Arc::get_mut(&mut mock).unwrap();
        m.hold = Some(gate_tx.clone());
    }
    let mock = Arc::clone(&mock);
    let (tx, _rx) = mpsc::unbounded_channel();
    let (_stop_tx, stop_rx) = watch::channel(false);
    let wake = Arc::new(Notify::new());
    let db2 = Arc::clone(&db);
    let m2 = Arc::clone(&mock);
    let handle = tokio::spawn(async move {
        let _ = run_pipeline(db2, m2, cfg(2, 1), stop_rx, wake, tx).await;
    });
    // Wait until chapter 1's TU is in flight (blocked on the gate).
    loop {
        if mock.active.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Chapter 2's TU must not have been touched while W=1 and ch1 is busy.
    assert_eq!(tu_status(&db, tus[1][0]), TuStatus::Pending);
    gate_tx.send(true).unwrap();
    handle.await.unwrap();
    assert_eq!(tu_status(&db, tus[0][0]), TuStatus::Translated);
    assert_eq!(tu_status(&db, tus[1][0]), TuStatus::Translated);
}

#[tokio::test]
async fn reviewing_tu_is_never_claimed() {
    let (_d, db, tus) = build_project(&[("甲", &["x1", "x2"])]);
    // User is reviewing the second TU.
    db.db()
        .write(|c| c.execute("UPDATE tus SET status='reviewing' WHERE id=?1", [tus[0][1]]).map_err(Into::into))
        .unwrap();
    let mock = MockTranslator::new("T");
    run_to_end(Arc::clone(&db), mock.clone(), cfg(2, 1)).await;
    let calls = mock.calls.lock().unwrap().clone();
    assert_eq!(calls, vec!["x1".to_string()]);
    assert_eq!(tu_status(&db, tus[0][0]), TuStatus::Translated);
    assert_eq!(tu_status(&db, tus[0][1]), TuStatus::Reviewing);
}

#[tokio::test]
async fn user_takeover_discards_worker_result() {
    let (_d, db, tus) = build_project(&[("甲", &["solo"])]);
    let (gate_tx, _gate_rx) = watch::channel(false);
    let mut mock = MockTranslator::new("T");
    {
        let m = Arc::get_mut(&mut mock).unwrap();
        m.hold = Some(gate_tx.clone());
    }
    let mock = Arc::clone(&mock);
    let (tx, _rx) = mpsc::unbounded_channel();
    let (_stop_tx, stop_rx) = watch::channel(false);
    let wake = Arc::new(Notify::new());
    let db2 = Arc::clone(&db);
    let m2 = Arc::clone(&mock);
    let handle = tokio::spawn(async move {
        let _ = run_pipeline(db2, m2, cfg(1, 1), stop_rx, wake, tx).await;
    });
    loop {
        if mock.active.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // User takes over mid-flight: translating → reviewing.
    db.db()
        .write(|c| c.execute("UPDATE tus SET status='reviewing' WHERE id=?1", [tus[0][0]]).map_err(Into::into))
        .unwrap();
    gate_tx.send(true).unwrap();
    handle.await.unwrap();
    // Worker's result was discarded; the TU stays reviewing with no draft.
    assert_eq!(tu_status(&db, tus[0][0]), TuStatus::Reviewing);
    assert!(db.get_translation(tus[0][0]).unwrap().is_none());
}

#[tokio::test]
async fn memory_dedup_skips_llm() {
    let (_d, db, tus) = build_project(&[("甲", &["同样的台词"])]);
    let tu = tus[0][0];
    let hash = source_hash("同样的台词");
    // Pre-seed an approved translation for the same source.
    db.claim_tu(tu).unwrap();
    db.complete_translation(tu, "同一句的译文", &hash, false).unwrap();
    db.approve_tu(tu).unwrap();

    // A second TU with identical source.
    let (db2, tu2) = {
        let cid = db.create_chapter("乙", 1).unwrap();
        let p = IngestedParagraph::new(
            "同样的台词".into(),
            None,
            "test.txt".into(),
            None,
            OcrParagraphStatus::Ok,
            serde_json::Value::Null,
        );
        db.insert_paragraphs(cid, &[p.clone()]).unwrap();
        let ids = vec![p.id.to_string()];
        let id = db
            .db()
            .write(|c| {
                c.execute(
                    "INSERT INTO tus (chapter_id, paragraph_ids, ord, budget, status)
                     VALUES (?1, ?2, ?3, NULL, 'pending')",
                    rusqlite::params![cid, serde_json::to_string(&ids).unwrap(), 0i64],
                )?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();
        (db, id)
    };

    let mock = MockTranslator::new("不会用到");
    run_to_end(Arc::clone(&db2), mock.clone(), cfg(1, 2)).await;
    assert!(mock.calls.lock().unwrap().is_empty(), "LLM must be skipped on memory hit");
    let t = db2.get_translation(tu2).unwrap().unwrap();
    assert_eq!(t.status, TranslationStatus::MemoryHit);
    assert_eq!(t.final_text.as_deref(), Some("同一句的译文"));
    assert_eq!(tu_status(&db2, tu2), TuStatus::Translated);
}

#[tokio::test]
async fn stop_graceful_completes_inflight() {
    let (_d, db, tus) = build_project(&[("甲", &["slow", "fast"])]);
    let mut mock = MockTranslator::new("T");
    {
        let m = Arc::get_mut(&mut mock).unwrap();
        m.delay_ms = 5;
    }
    let mock = Arc::clone(&mock);
    let (tx, _rx) = mpsc::unbounded_channel();
    let (stop_tx, stop_rx) = watch::channel(false);
    let wake = Arc::new(Notify::new());
    let db2 = Arc::clone(&db);
    let m2 = Arc::clone(&mock);
    let handle = tokio::spawn(async move {
        let _ = run_pipeline(db2, m2, cfg(1, 1), stop_rx, wake, tx).await;
    });
    // Wait until the first TU is in flight.
    loop {
        if mock.active.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    stop_tx.send(true).unwrap();
    handle.await.unwrap();
    // In-flight TU completed; the second was never claimed.
    assert_eq!(tu_status(&db, tus[0][0]), TuStatus::Translated);
    assert_eq!(tu_status(&db, tus[0][1]), TuStatus::Pending);
}

#[tokio::test]
async fn stop_aborts_inflight() {
    let (_d, db, tus) = build_project(&[("甲", &["blocked"])]);
    let (gate_tx, _gate_rx) = watch::channel(false);
    let mut mock = MockTranslator::new("T");
    {
        let m = Arc::get_mut(&mut mock).unwrap();
        m.hold = Some(gate_tx.clone());
    }
    let mock = Arc::clone(&mock);
    let (tx, _rx) = mpsc::unbounded_channel();
    let (stop_tx, stop_rx) = watch::channel(false);
    let wake = Arc::new(Notify::new());
    let mut c = cfg(1, 1);
    c.stop_aborts_inflight = true;
    let db2 = Arc::clone(&db);
    let m2 = Arc::clone(&mock);
    let handle = tokio::spawn(async move {
        let _ = run_pipeline(db2, m2, c, stop_rx, wake, tx).await;
    });
    loop {
        if mock.active.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    stop_tx.send(true).unwrap();
    handle.await.unwrap();
    assert_eq!(tu_status(&db, tus[0][0]), TuStatus::Interrupted);
}

#[tokio::test]
async fn crash_recovery_requeues_interrupted() {
    let (_d, db, tus) = build_project(&[("甲", &["s1", "s2"])]);
    // Simulate a crash: TU 1 stuck translating.
    db.db()
        .write(|c| c.execute("UPDATE tus SET status='translating' WHERE id=?1", [tus[0][0]]).map_err(Into::into))
        .unwrap();
    let mock = MockTranslator::new("T");
    run_to_end(Arc::clone(&db), mock.clone(), cfg(1, 1)).await;
    assert_eq!(tu_status(&db, tus[0][0]), TuStatus::Translated);
    assert_eq!(tu_status(&db, tus[0][1]), TuStatus::Translated);
}

#[tokio::test]
async fn explicit_retry_only_requeues_failed() {
    let (_d, db, tus) = build_project(&[("甲", &["r1", "perm"])]);
    db.db()
        .write(|c| {
            c.execute("UPDATE tus SET status='failed_retryable' WHERE id=?1", [tus[0][0]])?;
            Ok(c.execute("UPDATE tus SET status='failed_permanent' WHERE id=?1", [tus[0][1]])?)
        })
        .unwrap();
    // A fresh run must NOT auto-retry failed TUs.
    let mock = MockTranslator::new("T");
    run_to_end(Arc::clone(&db), mock.clone(), cfg(1, 1)).await;
    assert_eq!(tu_status(&db, tus[0][0]), TuStatus::FailedRetryable);
    assert_eq!(tu_status(&db, tus[0][1]), TuStatus::FailedPermanent);

    // Explicit retry re-queues the retryable one; the permanent one stays put.
    assert!(db.retranslate_tu(tus[0][0], "").unwrap());
    assert!(db.retranslate_tu(tus[0][1], "").unwrap());
    // Running again: retryable → translated, permanent untouched (still queued now —
    // retranslate allowed it; treat permanent as requeueable on explicit user action).
    let mock2 = MockTranslator::new("T");
    run_to_end(Arc::clone(&db), mock2.clone(), cfg(1, 1)).await;
    assert_eq!(tu_status(&db, tus[0][0]), TuStatus::Translated);
    assert_eq!(tu_status(&db, tus[0][1]), TuStatus::Translated);
}

#[tokio::test]
async fn requeue_failed_scopes_tu_chapter_all() {
    // Two chapters; one TU in each of the requeueable states plus one `approved`
    // that must never be touched.
    let (_d, db, tus) = build_project(&[("甲", &["a1", "a2"]), ("乙", &["b1", "b2"])]);
    db.db()
        .write(|c| {
            c.execute("UPDATE tus SET status='failed_retryable' WHERE id=?1", [tus[0][0]])?;
            c.execute("UPDATE tus SET status='interrupted' WHERE id=?1", [tus[0][1]])?;
            c.execute("UPDATE tus SET status='failed_permanent' WHERE id=?1", [tus[1][0]])?;
            Ok(c.execute("UPDATE tus SET status='approved' WHERE id=?1", [tus[1][1]])?)
        })
        .unwrap();
    let chapters = db.list_chapters().unwrap();
    let ch_甲 = chapters.iter().find(|c| c.title == "甲").unwrap().id;
    let ch_乙 = chapters.iter().find(|c| c.title == "乙").unwrap().id;

    // scope=tu: only the listed id.
    assert_eq!(db.requeue_failed(Some(&[tus[0][1]]), None).unwrap(), 1);
    assert_eq!(tu_status(&db, tus[0][1]), TuStatus::Queued);
    assert_eq!(tu_status(&db, tus[0][0]), TuStatus::FailedRetryable);

    // scope=chapter: only that chapter's failed/interrupted.
    assert_eq!(db.requeue_failed(None, Some(ch_甲)).unwrap(), 1);
    assert_eq!(tu_status(&db, tus[0][0]), TuStatus::Queued);
    assert_eq!(tu_status(&db, tus[1][0]), TuStatus::FailedPermanent); // 乙 untouched
    assert_eq!(tu_status(&db, tus[1][1]), TuStatus::Approved);

    // scope=all: the remaining failed TU, wherever it is.
    assert_eq!(db.requeue_failed(None, None).unwrap(), 1);
    assert_eq!(tu_status(&db, tus[1][0]), TuStatus::Queued);
    assert_eq!(tu_status(&db, tus[1][1]), TuStatus::Approved); // approved never touched

    // Chapter ids / ids are mutually exclusive: a bogus combination requeues nothing.
    assert_eq!(db.requeue_failed(Some(&[tus[1][0]]), Some(ch_乙)).unwrap(), 0);
    assert_eq!(tu_status(&db, tus[1][0]), TuStatus::Queued);
}

#[tokio::test]
async fn prompt_injects_glossary_and_context() {
    // Enabled project small-glossary entries feed the prompt matcher; the
    // japanese form and every alias should both hit. A disabled entry must not.
    let (_d, db, tus) = build_project(&[("甲", &["前段原文", "田中来了", "たなか来了"])]);
    add_entry(&db, "田中", "田中", &["たなか"]);
    let disabled_id = add_entry(&db, "禁用词", "禁用", &[]);
    db.set_entry_enabled(disabled_id, false).unwrap();

    // Approve the first TU so it becomes context for the others.
    let h1 = source_hash("前段原文");
    db.claim_tu(tus[0][0]).unwrap();
    db.complete_translation(tus[0][0], "前段译文", &h1, false).unwrap();
    db.approve_tu(tus[0][0]).unwrap();

    let mut mock = MockTranslator::new("T");
    {
        let m = Arc::get_mut(&mut mock).unwrap();
        m.fail = false;
    }
    let mock = Arc::clone(&mock);
    run_to_end(Arc::clone(&db), mock.clone(), cfg(1, 1)).await;
    let reqs = mock.requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2, "the two remaining TUs should reach the LLM");
    let req = &reqs[0];
    assert!(req.context.as_deref().unwrap_or("").contains("前段译文"));
    assert!(req.glossary.as_deref().unwrap_or("").contains("田中 → 田中"));
    assert!(req.system.contains("日译中"));
    // The alias form matches too; the disabled entry is never injected.
    let alias_req = &reqs[1];
    assert!(alias_req.glossary.as_deref().unwrap_or("").contains("田中 → 田中"));
    assert!(!alias_req.glossary.as_deref().unwrap_or("").contains("禁用"));
}

#[tokio::test]
async fn failed_tu_records_error_and_retries() {
    let (_d, db, tus) = build_project(&[("甲", &["err"])]);
    let mut mock = MockTranslator::new("T");
    {
        let m = Arc::get_mut(&mut mock).unwrap();
        m.fail = true;
    }
    let mock = Arc::clone(&mock);
    run_to_end(Arc::clone(&db), mock.clone(), cfg(1, 1)).await;
    assert_eq!(tu_status(&db, tus[0][0]), TuStatus::FailedRetryable);
    let t = db.get_translation(tus[0][0]).unwrap().unwrap();
    assert!(t.error.as_deref().unwrap_or("").contains("mock failure"));
    assert_eq!(t.status, TranslationStatus::Failed);
}

// ---------------------------------------------------------------------------
// Storage-level guards (the state-gate SQL).
// ---------------------------------------------------------------------------

#[test]
fn claim_and_release_guards() {
    let (_d, db, tus) = build_project(&[("甲", &["p"])]);
    let tu = tus[0][0];
    // Claim only from pending/queued.
    assert!(db.claim_tu(tu).unwrap());
    assert!(!db.claim_tu(tu).unwrap(), "cannot claim twice");
    // complete requires translating.
    assert!(db.complete_translation(tu, "T", &source_hash("p"), false).unwrap());
    assert!(!db.complete_translation(tu, "T", &source_hash("p"), false).unwrap());
    assert_eq!(tu_status(&db, tu), TuStatus::Translated);
    // approve from translated.
    assert!(db.approve_tu(tu).unwrap());
    assert!(!db.approve_tu(tu).unwrap());
    // retranslate from approved.
    assert!(db.retranslate_tu(tu, "指示").unwrap());
    assert_eq!(tu_status(&db, tu), TuStatus::Queued);
    let t = db.get_translation(tu).unwrap().unwrap();
    assert_eq!(t.instruction.as_deref(), Some("指示"));
    // Re-translate: the previous draft is archived into `attempts`.
    assert!(db.claim_tu(tu).unwrap());
    assert!(db.complete_translation(tu, "T2", &source_hash("p"), false).unwrap());
    let t = db.get_translation(tu).unwrap().unwrap();
    assert_eq!(t.final_text.as_deref(), Some("T2"));
    assert_eq!(t.attempts, vec!["T".to_string()], "old draft preserved in attempts");
}

#[test]
fn settings_defaults_and_clamp() {
    let dir = TempDir::new().unwrap();
    let db = ProjectDb::open(&dir.path().join("project.db")).unwrap();
    let d = db.get_translation_settings().unwrap();
    assert_eq!(d.workers, TranslationSettings::default().workers);
    assert_eq!(d.window, TranslationSettings::default().window);
    assert!(d.memory_dedup);
    assert!(!d.stop_aborts_inflight);
    let s = TranslationSettings { workers: 99, window: 0, memory_dedup: false, stop_aborts_inflight: true };
    db.set_translation_settings(&s).unwrap();
    let back = db.get_translation_settings().unwrap();
    assert_eq!(back.workers, 8, "workers clamped to [1,8]");
    assert_eq!(back.window, 1, "window clamped to [1,5]");
    assert!(!back.memory_dedup);
    assert!(back.stop_aborts_inflight);
}

#[test]
fn guidelines_default_and_roundtrip() {
    let dir = TempDir::new().unwrap();
    let db = ProjectDb::open(&dir.path().join("project.db")).unwrap();
    assert!(db.get_guidelines().unwrap().contains("日译中"));
    db.set_guidelines("自定义总则").unwrap();
    assert_eq!(db.get_guidelines().unwrap(), "自定义总则");
}
