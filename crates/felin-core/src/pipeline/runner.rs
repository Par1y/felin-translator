//! Scheduler + worker pool for the translation pipeline.
//!
//! One scheduler task owns the chapter-activation window `W` and feeds eligible
//! TU ids into a bounded channel; `N` workers pull, CAS-claim from the DB (the
//! single source of truth), translate, and write back. See [`super`] for the
//! concurrency model.

use super::{source_hash, PipelineEvent, RunConfig, Translator};
use crate::error::{Error, Result};
use crate::names::Matcher;
use crate::pipeline::prompt::{build_tu_request, glossary_block};
use crate::storage::ProjectDb;
use crate::types::GlossaryEntry;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Notify};

/// One worker's outcome; events are emitted by the caller.
enum Outcome {
    Done { memory_hit: bool },
    Failed { error: String },
    /// Stop-with-abort: the TU was marked `interrupted`.
    Aborted,
    /// The TU left `translating` while we worked (user took over): discard.
    Discarded,
}

/// Compiled glossary data for prompt injection. Built from the *project's small
/// glossary* (enabled entries only): the canonical japanese form plus every
/// alias feed the matcher; the lookup maps the entry id to the canonical form
/// and its Chinese rendering for the injected block.
struct GlossaryData {
    matcher: Matcher,
    lookup: HashMap<i64, (String, Option<String>)>,
}

fn build_glossary(entries: &[GlossaryEntry]) -> Result<Option<GlossaryData>> {
    if entries.is_empty() {
        return Ok(None);
    }
    let mut forms: Vec<(String, i64)> = Vec::new();
    let mut lookup: HashMap<i64, (String, Option<String>)> = HashMap::new();
    for e in entries {
        forms.push((e.japanese.clone(), e.id));
        for a in &e.aliases {
            if !a.trim().is_empty() {
                forms.push((a.clone(), e.id));
            }
        }
        lookup.insert(e.id, (e.japanese.clone(), e.chinese.clone()));
    }
    let matcher = Matcher::build(&forms)?;
    Ok(Some(GlossaryData { matcher, lookup }))
}

/// Run the pipeline to completion or stop. Crash recovery happens first: stale
/// `translating` → `interrupted`, then `interrupted` → `queued` (resume).
/// Glossary data for prompt injection is fetched from the project DB itself
/// ([`ProjectDb::matcher_entries`]) — the caller never passes it in.
pub async fn run<T: Translator + 'static>(
    db: Arc<ProjectDb>,
    translator: Arc<T>,
    cfg: RunConfig,
    stop: &mut watch::Receiver<bool>,
    wake: Arc<Notify>,
    events: mpsc::UnboundedSender<PipelineEvent>,
) -> Result<()> {
    db.recover_interrupted()?;
    db.requeue_interrupted()?;

    let guidelines = db.get_guidelines()?;
    let glossary = Arc::new(build_glossary(&db.matcher_entries()?)?);

    let workers = cfg.workers.max(1);
    let queue = cfg.queue_capacity.max(workers);
    let cfg = RunConfig { queue_capacity: queue, ..cfg };

    let (feed_tx, feed_rx) = mpsc::channel::<i64>(queue);
    let (free_tx, free_rx) = mpsc::channel::<()>(workers);
    // `mpsc::Receiver` can't be cloned; the pool shares it behind a mutex.
    // Only one worker blocks on `recv()` at a time — correct (each id goes to
    // exactly one worker). The guard must be *scoped* to the recv: a `while
    // let` scrutinee's temporaries live until the end of the body, which would
    // hold the lock across `process_one` and serialize the whole pool.
    let feed = Arc::new(tokio::sync::Mutex::new(feed_rx));

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let db = Arc::clone(&db);
        let translator = Arc::clone(&translator);
        let cfg = cfg.clone();
        let guidelines = guidelines.clone();
        let glossary = Arc::clone(&glossary);
        let feed = Arc::clone(&feed);
        let free_tx = free_tx.clone();
        let events = events.clone();
        let stop = stop.clone();
        handles.push(tokio::spawn(worker_loop(
            db, translator, cfg, guidelines, glossary, feed, free_tx, events, stop,
        )));
    }
    drop(free_tx);

    scheduler(db, cfg, feed_tx, free_rx, events.clone(), stop.clone(), wake).await;

    for h in handles {
        let _ = h.await;
    }
    Ok(())
}

/// The scheduler: maintains the activation window, feeds eligible TUs, and
/// decides when the run is finished (nothing outstanding + nothing eligible).
async fn scheduler(
    db: Arc<ProjectDb>,
    cfg: RunConfig,
    feed: mpsc::Sender<i64>,
    mut free_rx: mpsc::Receiver<()>,
    events: mpsc::UnboundedSender<PipelineEvent>,
    mut stop: watch::Receiver<bool>,
    wake: Arc<Notify>,
) {
    let total = db.count_tus().unwrap_or(0) as usize;
    let _ = events.send(PipelineEvent::Started { total_tus: total });

    let mut outstanding = 0usize;
    outstanding += fill(&db, &cfg, &feed, outstanding).await;

    loop {
        if outstanding == 0 && !has_any_eligible(&db, &cfg).unwrap_or(false) {
            let _ = events.send(PipelineEvent::Finished { total_tus: total });
            // `feed` drops as we return, closing the workers' channel.
            return;
        }
        tokio::select! {
            biased;
            _ = stop.changed() => {}
            _ = wake.notified() => {}
            _ = free_rx.recv() => {
                outstanding = outstanding.saturating_sub(1);
            }
        }
        if *stop.borrow() {
            break;
        }
        outstanding += fill(&db, &cfg, &feed, outstanding).await;
    }

    drop(feed);
    let _ = events.send(PipelineEvent::Stopped);
}

/// Push newly-eligible TU ids into `feed` up to the free capacity; returns how
/// many were enqueued.
async fn fill(
    db: &ProjectDb,
    cfg: &RunConfig,
    feed: &mpsc::Sender<i64>,
    outstanding: usize,
) -> usize {
    let slots = cfg.queue_capacity.saturating_sub(outstanding);
    if slots == 0 {
        return 0;
    }
    let Ok(active) = db.active_chapter_ids(cfg.window) else { return 0 };
    let Ok(eligible) = db.next_eligible_tus(&active, slots) else { return 0 };
    let mut sent = 0usize;
    for id in eligible {
        if feed.send(id).await.is_err() {
            break;
        }
        sent += 1;
    }
    sent
}

/// Any claimable TU anywhere in the window?
fn has_any_eligible(db: &ProjectDb, cfg: &RunConfig) -> Result<bool> {
    let active = db.active_chapter_ids(cfg.window)?;
    Ok(!db.next_eligible_tus(&active, 1)?.is_empty())
}

/// One worker: pull TUs from the feed, process each, signal free.
async fn worker_loop<T: Translator + 'static>(
    db: Arc<ProjectDb>,
    translator: Arc<T>,
    cfg: RunConfig,
    guidelines: String,
    glossary: Arc<Option<GlossaryData>>,
    feed: Arc<tokio::sync::Mutex<mpsc::Receiver<i64>>>,
    free_tx: mpsc::Sender<()>,
    events: mpsc::UnboundedSender<PipelineEvent>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        // Scope the mutex guard so it drops before `process_one`: a `while
        // let` holds scrutinee temporaries across the whole body, which would
        // serialize the pool behind a single worker.
        let tu_id = {
            let mut rx = feed.lock().await;
            rx.recv().await
        };
        let Some(tu_id) = tu_id else { break };
        process_one(&db, translator.as_ref(), &cfg, &guidelines, &glossary, &events, &mut stop, tu_id).await;
        let _ = free_tx.send(()).await;
        // Graceful stop: finish the current TU, then exit without pulling more.
        // The feed is buffered and the scheduler drops it at its next poll, so
        // without this check a worker would drain remaining ids after a stop.
        if *stop.borrow() {
            break;
        }
    }
}

async fn process_one<T: Translator + 'static>(
    db: &ProjectDb,
    translator: &T,
    cfg: &RunConfig,
    guidelines: &str,
    glossary: &Option<GlossaryData>,
    events: &mpsc::UnboundedSender<PipelineEvent>,
    stop: &mut watch::Receiver<bool>,
    tu_id: i64,
) {
    // CAS claim; if we lose, another path (retry / user) already owns the TU.
    if !db.claim_tu(tu_id).unwrap_or(false) {
        return;
    }
    let _ = events.send(PipelineEvent::TuStart { tu_id });

    let outcome = match run_one(db, translator, cfg, guidelines, glossary, stop, tu_id).await {
        Ok(o) => o,
        Err(e) => {
            // DB-level failure: release the claim so the TU isn't stuck translating.
            let _ = db.interrupt_tu(tu_id);
            Outcome::Failed { error: format!("{e}") }
        }
    };
    match outcome {
        Outcome::Done { memory_hit } => {
            let _ = events.send(PipelineEvent::TuDone { tu_id, memory_hit });
        }
        Outcome::Failed { error } => {
            let _ = events.send(PipelineEvent::TuFailed { tu_id, error });
        }
        Outcome::Aborted | Outcome::Discarded => {}
    }
}

/// Translate one already-claimed TU. All DB writes that move the TU are
/// *conditional* on it still being `translating`, so a user takeover is never
/// overwritten.
async fn run_one<T: Translator + 'static>(
    db: &ProjectDb,
    translator: &T,
    cfg: &RunConfig,
    guidelines: &str,
    glossary: &Option<GlossaryData>,
    stop: &mut watch::Receiver<bool>,
    tu_id: i64,
) -> Result<Outcome> {
    let source = db.tu_source(tu_id)?;
    if source.trim().is_empty() {
        db.fail_translation(tu_id, "TU 无原文", true)?;
        return Ok(Outcome::Failed { error: "TU 无原文".into() });
    }
    let hash = source_hash(&source);

    // Translation memory: skip the LLM for a repeated, already-approved source.
    if cfg.memory_dedup {
        if let Some((_, memorized)) = db.find_memory_hit(&hash)? {
            return match db.complete_translation(tu_id, memorized.trim(), &hash, true)? {
                true => Ok(Outcome::Done { memory_hit: true }),
                false => Ok(Outcome::Discarded),
            };
        }
    }

    let tu = db
        .get_tu(tu_id)?
        .ok_or_else(|| Error::llm("TU disappeared mid-flight"))?;
    let context = db.prev_approved_context(tu.chapter_id, tu_id)?;
    let gloss = glossary.as_ref().and_then(|g| {
        let hits = g.matcher.find_hits(&source);
        glossary_block(&hits, &g.lookup)
    });
    let instruction = db
        .get_translation(tu_id)?
        .and_then(|t| t.instruction)
        .filter(|s| !s.trim().is_empty());
    let req = build_tu_request(
        guidelines.to_string(),
        cfg.guidelines_max_chars,
        instruction,
        gloss,
        context,
        cfg.context_max_chars,
        source,
        cfg.system_template.clone(),
        cfg.user_template.clone(),
    );

    let result = if cfg.stop_aborts_inflight {
        tokio::select! {
            r = translator.translate(&req) => r,
            _ = stop.wait_for(|v| *v) => {
                let _ = db.interrupt_tu(tu_id);
                return Ok(Outcome::Aborted);
            }
        }
    } else {
        translator.translate(&req).await
    };

    match result {
        Ok(text) if !text.trim().is_empty() => match db.complete_translation(tu_id, text.trim(), &hash, false)? {
            true => Ok(Outcome::Done { memory_hit: false }),
            false => Ok(Outcome::Discarded),
        },
        Ok(_) => {
            let msg = "空回复".to_string();
            db.fail_translation(tu_id, &msg, false)?;
            Ok(Outcome::Failed { error: msg })
        }
        Err(e) => {
            let msg = format!("{e}");
            db.fail_translation(tu_id, &msg, false)?;
            Ok(Outcome::Failed { error: msg })
        }
    }
}
