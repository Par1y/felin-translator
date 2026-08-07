//! Per-project database (`project.db`): chapters, paragraphs, TUs, translations,
//! extracted names, overrides, exports, and settings.
//!
//! The foundation milestone implements the read/write paths exercised by OCR
//! ingest (settings, chapters, paragraphs). The remaining tables exist in the
//! schema for later milestones.

use crate::error::{Error, Result};
use crate::seg::{aggregate, clean_text, ChapterRecognizer};
use crate::storage::db::{Db, DbTuning};
use crate::storage::migrations::Migration;
use crate::types::{
    Chapter, ChapterStatus, ExtractedName, ExtractedNameStatus, GlossaryEntry, IngestedParagraph,
    OcrSettings, Paragraph, Translation, TranslationExport, TranslationSettings, Tu, TuStatus,
    TuWithTranslation,
};
use crate::util::now_iso8601;
use rusqlite::OptionalExtension;
use std::collections::HashMap;
use std::path::Path;
use uuid::Uuid;

/// Result of a segmentation pass.
#[derive(Debug, Clone, Copy)]
pub struct SegmentOutcome {
    pub chapters: usize,
    pub tus: usize,
}

/// Ordered migration set for the project DB. Append new migrations here.
pub const PROJECT_MIGRATIONS: &[Migration] = &[
    Migration { version: 1, sql: include_str!("migrations/project/0001_init.sql") },
    Migration { version: 2, sql: include_str!("migrations/project/0002_translations_unique.sql") },
    Migration { version: 3, sql: include_str!("migrations/project/0003_glossary_entries.sql") },
    Migration { version: 4, sql: include_str!("migrations/project/0004_tu_source_override.sql") },
    Migration { version: 5, sql: include_str!("migrations/project/0005_remove_aliases.sql") },
    Migration { version: 6, sql: include_str!("migrations/project/0006_extracted_tags.sql") },
];

/// Typed wrapper over a single project's database.
#[derive(Debug)]
pub struct ProjectDb {
    db: Db,
}

impl ProjectDb {
    /// Open (creating + migrating, backing up before any upgrade) the project DB
    /// at `db_path` (typically `projects/<slug>/project.db`).
    pub fn open(db_path: &Path) -> Result<Self> {
        Self::open_with(db_path, DbTuning::default())
    }

    /// Open with explicit connection tuning.
    pub fn open_with(db_path: &Path, tuning: DbTuning) -> Result<Self> {
        Ok(Self { db: Db::open(db_path, PROJECT_MIGRATIONS, true, tuning)? })
    }

    /// Access the underlying handle for queries not yet wrapped here.
    pub fn db(&self) -> &Db {
        &self.db
    }

    // ----- settings -------------------------------------------------------

    /// Upsert a key/value setting.
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.db.write(|c| {
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![key, value],
            )?;
            Ok(())
        })
    }

    /// Read a setting value, if set.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.db.read(|c| {
            c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()
            .map(Option::flatten)
            .map_err(Into::into)
        })
    }

    /// Persist the project's display name (the `project_name` setting, also
    /// mirrored in `project.json`). Renaming only touches the display name —
    /// the disk directory / slug are never changed.
    pub fn set_project_name(&self, name: &str) -> Result<()> {
        self.set_setting("project_name", name)
    }

    // ----- chapters -------------------------------------------------------

    /// Create a chapter at the given ordinal; returns its id.
    pub fn create_chapter(&self, title: &str, ord: i64) -> Result<i64> {
        self.db.write(|c| {
            c.execute(
                "INSERT INTO chapters (title, ord, status) VALUES (?1, ?2, ?3)",
                rusqlite::params![title, ord, ChapterStatus::Pending],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    /// Next free chapter ordinal (max + 1, or 0).
    pub fn next_chapter_ord(&self) -> Result<i64> {
        self.db.read(|c| Ok(c.query_row("SELECT COALESCE(MAX(ord), -1) + 1 FROM chapters", [], |r| r.get(0))?))
    }

    /// Return the id of the chapter titled `title`, creating it at the next
    /// ordinal if it does not exist. Atomic: the read-check + ordinal + insert
    /// all run inside one write transaction, so concurrent callers can't create
    /// duplicate chapters.
    pub fn get_or_create_chapter(&self, title: &str) -> Result<i64> {
        self.db.write(|conn| {
            let tx = conn.transaction()?;
            let existing: Option<i64> = tx
                .query_row("SELECT id FROM chapters WHERE title = ?1", [title], |r| r.get(0))
                .optional()?;
            let id = match existing {
                Some(id) => id,
                None => {
                    let ord: i64 =
                        tx.query_row("SELECT COALESCE(MAX(ord), -1) + 1 FROM chapters", [], |r| r.get(0))?;
                    tx.execute(
                        "INSERT INTO chapters (title, ord, status) VALUES (?1, ?2, ?3)",
                        rusqlite::params![title, ord, ChapterStatus::Pending],
                    )?;
                    tx.last_insert_rowid()
                }
            };
            tx.commit()?;
            Ok(id)
        })
    }

    /// All chapters, ordered.
    pub fn list_chapters(&self) -> Result<Vec<Chapter>> {
        self.db.read(|c| {
            let mut stmt = c.prepare("SELECT id, title, ord, status FROM chapters ORDER BY ord")?;
            let rows = stmt.query_map([], |r| {
                Ok(Chapter { id: r.get(0)?, title: r.get(1)?, ord: r.get(2)?, status: r.get(3)? })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    // ----- paragraphs -----------------------------------------------------

    /// Append ingested paragraphs to `chapter_id`. Ordinals continue from the
    /// chapter's current max. One transaction, one prepared statement reused.
    /// Returns the number inserted.
    pub fn insert_paragraphs(&self, chapter_id: i64, paras: &[IngestedParagraph]) -> Result<usize> {
        if paras.is_empty() {
            return Ok(0);
        }
        self.db.write(|conn| {
            let tx = conn.transaction()?;
            let mut ord: i64 = tx.query_row(
                "SELECT COALESCE(MAX(ord), -1) FROM paragraphs WHERE chapter_id = ?1",
                [chapter_id],
                |r| r.get(0),
            )?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO paragraphs
                       (id, chapter_id, ord, text, page_num, page_score, ocr_status, ocr_meta, source_file)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )?;
                for p in paras {
                    ord += 1;
                    let meta = if p.ocr_meta.is_null() {
                        None
                    } else {
                        Some(serde_json::to_string(&p.ocr_meta)?)
                    };
                    stmt.execute(rusqlite::params![
                        p.id.to_string(),
                        chapter_id,
                        ord,
                        p.text,
                        p.page_num,
                        p.page_score,
                        p.ocr_status,
                        meta,
                        p.source_file,
                    ])?;
                }
            }
            tx.commit()?;
            Ok(paras.len())
        })
    }

    /// All paragraphs in a chapter, ordered.
    pub fn list_paragraphs(&self, chapter_id: i64) -> Result<Vec<Paragraph>> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, chapter_id, ord, text, page_num, page_score, ocr_status, ocr_meta, source_file
                 FROM paragraphs WHERE chapter_id = ?1 ORDER BY ord",
            )?;
            let rows = stmt.query_map([chapter_id], row_to_paragraph)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    /// Total paragraph count across all chapters.
    pub fn count_paragraphs(&self) -> Result<i64> {
        self.db.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM paragraphs", [], |r| r.get(0))?))
    }

    // ----- translation units ---------------------------------------------

    /// All TUs in a chapter, ordered.
    pub fn list_tus(&self, chapter_id: i64) -> Result<Vec<Tu>> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, chapter_id, paragraph_ids, ord, budget, status
                 FROM tus WHERE chapter_id = ?1 ORDER BY ord",
            )?;
            let rows = stmt.query_map([chapter_id], row_to_tu)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    /// Total TU count across all chapters.
    pub fn count_tus(&self) -> Result<i64> {
        self.db.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM tus", [], |r| r.get(0))?))
    }

    // ----- translation settings (GUI user options) --------------------------

    /// Current translation settings, falling back to defaults for unset keys.
    pub fn get_translation_settings(&self) -> Result<TranslationSettings> {
        let d = TranslationSettings::default();
        Ok(TranslationSettings {
            workers: parse_i64(self.get_setting("workers")?.as_deref(), d.workers, 1, 8),
            window: parse_i64(self.get_setting("window")?.as_deref(), d.window, 1, 5),
            memory_dedup: parse_bool(self.get_setting("memory_dedup")?.as_deref(), d.memory_dedup),
            stop_aborts_inflight: parse_bool(
                self.get_setting("stop_aborts_inflight")?.as_deref(),
                d.stop_aborts_inflight,
            ),
        })
    }

    /// Persist translation settings.
    pub fn set_translation_settings(&self, s: &TranslationSettings) -> Result<()> {
        self.set_setting("workers", &s.workers.to_string())?;
        self.set_setting("window", &s.window.to_string())?;
        self.set_setting("memory_dedup", &s.memory_dedup.to_string())?;
        self.set_setting("stop_aborts_inflight", &s.stop_aborts_inflight.to_string())?;
        Ok(())
    }

    /// The project's 总则 (system prompt), falling back to the default template.
    pub fn get_guidelines(&self) -> Result<String> {
        Ok(self.get_setting("guidelines")?.unwrap_or_else(crate::pipeline::default_guidelines))
    }

    /// Persist the project 总则.
    pub fn set_guidelines(&self, text: &str) -> Result<()> {
        self.set_setting("guidelines", text)
    }

    /// Current OCR import settings, falling back to defaults for unset keys.
    pub fn get_ocr_settings(&self) -> Result<OcrSettings> {
        let d = OcrSettings::default();
        Ok(OcrSettings {
            batch_workers: parse_i64(self.get_setting("batch_workers")?.as_deref(), d.batch_workers, 1, 16),
            batch_recursive: parse_bool(self.get_setting("batch_recursive")?.as_deref(), d.batch_recursive),
        })
    }

    /// Persist OCR import settings.
    pub fn set_ocr_settings(&self, s: &OcrSettings) -> Result<()> {
        self.set_setting("batch_workers", &s.batch_workers.to_string())?;
        self.set_setting("batch_recursive", &s.batch_recursive.to_string())?;
        Ok(())
    }

    // ----- TU status transitions (pipeline state gate) ----------------------

    /// Atomically claim a TU for translation: only `pending`/`queued` may be
    /// taken, and only one caller wins. `reviewing`/`approved`/`exported` TUs
    /// are never touched by the pipeline.
    pub fn claim_tu(&self, id: i64) -> Result<bool> {
        self.db.write(|c| {
            let n = c.execute(
                "UPDATE tus SET status = 'translating' WHERE id = ?1 AND status IN ('pending','queued')",
                [id],
            )?;
            Ok(n == 1)
        })
    }

    /// Atomically write a translation result and release its TU — but only if
    /// the TU is still `translating`. If the user meanwhile moved it (e.g. to
    /// `reviewing`), the result is discarded and `Ok(false)` returned. This is
    /// the anchor of the "at most one writer per TU" invariant.
    pub fn complete_translation(
        &self,
        tu_id: i64,
        draft: &str,
        source_hash: &str,
        memory: bool,
    ) -> Result<bool> {
        self.db.write(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let released = tx.execute(
                "UPDATE tus SET status = 'translated' WHERE id = ?1 AND status = 'translating'",
                [tu_id],
            )? == 1;
            if !released {
                tx.rollback()?;
                return Ok(false);
            }
            upsert_translation(&tx, tu_id, draft, source_hash, memory)?;
            tx.commit()?;
            Ok(true)
        })
    }

    /// Atomically record a translation failure and release the TU to
    /// `failed_retryable` (or `failed_permanent` when `permanent`) — never
    /// touching `reviewing`/`approved` TUs.
    pub fn fail_translation(&self, tu_id: i64, error: &str, permanent: bool) -> Result<bool> {
        let to = if permanent { "failed_permanent" } else { "failed_retryable" };
        self.db.write(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let released = tx.execute(
                &format!("UPDATE tus SET status = '{to}' WHERE id = ?1 AND status = 'translating'"),
                [tu_id],
            )? == 1;
            if !released {
                tx.rollback()?;
                return Ok(false);
            }
            upsert_error(&tx, tu_id, error)?;
            tx.commit()?;
            Ok(true)
        })
    }

    /// Abort an in-flight TU (stop-with-abort / crash path): `translating` →
    /// `interrupted`.
    pub fn interrupt_tu(&self, tu_id: i64) -> Result<bool> {
        self.db.write(|c| {
            let n = c.execute(
                "UPDATE tus SET status = 'interrupted' WHERE id = ?1 AND status = 'translating'",
                [tu_id],
            )?;
            Ok(n == 1)
        })
    }

    /// Crash recovery: any TU left `translating` at startup becomes `interrupted`.
    pub fn recover_interrupted(&self) -> Result<usize> {
        self.db.write(|c| {
            let n = c.execute("UPDATE tus SET status = 'interrupted' WHERE status = 'translating'", [])?;
            Ok(n)
        })
    }

    /// Re-queue previously interrupted TUs on a fresh run (resume after stop /
    /// crash). Failed TUs are left for the explicit retry button.
    pub fn requeue_interrupted(&self) -> Result<usize> {
        self.db.write(|c| {
            let n = c.execute("UPDATE tus SET status = 'queued' WHERE status = 'interrupted'", [])?;
            Ok(n)
        })
    }

    /// Re-queue failed/interrupted TUs for the explicit retry button, scoped to
    /// specific ids, a chapter, or the whole project (the plan's
    /// `retry_translation(scope=tu|chapter|all)`). Only `failed_*`/`interrupted`
    /// are touched — finished or in-flight TUs are left alone. Pass `(Some(ids),
    /// None)` for scope=tu, `(None, Some(chapter_id))` for scope=chapter, or
    /// `(None, None)` for scope=all; any other combination is a caller bug and
    /// requeues nothing.
    pub fn requeue_failed(&self, ids: Option<&[i64]>, chapter_id: Option<i64>) -> Result<usize> {
        use rusqlite::params_from_iter;
        let (sql, vals) = match (ids, chapter_id) {
            (Some(ids), None) => {
                if ids.is_empty() {
                    return Ok(0);
                }
                let ph = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
                (
                    format!(
                        "UPDATE tus SET status = 'queued' WHERE id IN ({ph}) AND status IN
                         ('failed_retryable','failed_permanent','interrupted')"
                    ),
                    ids.to_vec(),
                )
            }
            (None, Some(ch)) => (
                "UPDATE tus SET status = 'queued' WHERE chapter_id = ?1 AND status IN
                 ('failed_retryable','failed_permanent','interrupted')"
                    .to_string(),
                vec![ch],
            ),
            (None, None) => (
                "UPDATE tus SET status = 'queued' WHERE status IN
                 ('failed_retryable','failed_permanent','interrupted')"
                    .to_string(),
                vec![],
            ),
            (Some(_), Some(_)) => return Ok(0),
        };
        self.db.write(|c| Ok(c.execute(&sql, params_from_iter(vals))?))
    }

    /// Chapter ids that currently have ≥1 claimable TU, first `window` by ord —
    /// the scheduler's activation window.
    pub fn active_chapter_ids(&self, window: usize) -> Result<Vec<i64>> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT DISTINCT t.chapter_id FROM tus t JOIN chapters c ON t.chapter_id = c.id
                 WHERE t.status IN ('pending','queued')
                 ORDER BY c.ord LIMIT ?1",
            )?;
            let rows = stmt.query_map([window as i64], |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    /// One TU row, if present.
    pub fn get_tu(&self, tu_id: i64) -> Result<Option<Tu>> {
        self.db.read(|c| {
            c.query_row(
                "SELECT id, chapter_id, paragraph_ids, ord, budget, status
                 FROM tus WHERE id = ?1",
                [tu_id],
                row_to_tu,
            )
            .optional()
            .map_err(Into::into)
        })
    }

    /// Human "approve" transition: `translated`/`reviewing` → `approved`.
    pub fn approve_tu(&self, tu_id: i64) -> Result<bool> {
        self.db.write(|c| {
            let n = c.execute(
                "UPDATE tus SET status = 'approved' WHERE id = ?1 AND status IN ('translated','reviewing')",
                [tu_id],
            )?;
            Ok(n == 1)
        })
    }

    /// Re-translate: move a finished/failed TU back to `queued` with (optional)
    /// per-item instruction, atomically. Returns false if the TU is mid-flight
    /// (`pending`/`queued`/`translating`), which is not eligible.
    pub fn retranslate_tu(&self, tu_id: i64, instruction: &str) -> Result<bool> {
        self.db.write(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let requeued = tx.execute(
                "UPDATE tus SET status = 'queued' WHERE id = ?1 AND status IN
                 ('translated','reviewing','approved','interrupted','failed_retryable','failed_permanent')",
                [tu_id],
            )? == 1;
            if !requeued {
                tx.rollback()?;
                return Ok(false);
            }
            upsert_instruction(&tx, tu_id, instruction)?;
            tx.commit()?;
            Ok(true)
        })
    }

    /// Persist (or clear) a per-item instruction without re-queuing.
    pub fn set_tu_instruction(&self, tu_id: i64, instruction: &str) -> Result<()> {
        self.db.write(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            upsert_instruction(&tx, tu_id, instruction)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Persist the user's hand-edited 译文 for a TU. Editing an already-approved
    /// or failed/interrupted TU demotes it back to `reviewing` (approved work is
    /// never silently kept in lockstep with a newer draft). Writes into the
    /// translation row's `final_text`, creating the row if absent, and clears any
    /// recorded error.
    pub fn set_translation_text(&self, tu_id: i64, final_text: &str) -> Result<bool> {
        self.db.write(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let demoted = tx.execute(
                "UPDATE tus SET status = 'reviewing' WHERE id = ?1 AND status IN
                 ('approved','exported','interrupted','failed_retryable','failed_permanent')",
                [tu_id],
            )? == 1;
            upsert_final_text(&tx, tu_id, final_text)?;
            tx.commit()?;
            Ok(demoted)
        })
    }

    /// Batch re-translate: requeue several finished/failed TUs in one transaction,
    /// optionally stamping each with an extra instruction. The instruction is only
    /// written when `Some` (pass `Some("")` to clear it). Returns how many TUs
    /// were actually requeued.
    pub fn retranslate_tus(&self, ids: &[i64], instruction: Option<&str>) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.db.write(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let mut n = 0usize;
            let mut stmt = tx.prepare(
                "UPDATE tus SET status = 'queued' WHERE id = ?1 AND status IN
                 ('translated','reviewing','approved','interrupted','failed_retryable','failed_permanent')",
            )?;
            for id in ids {
                if stmt.execute([id])? == 1 {
                    n += 1;
                    if let Some(instr) = instruction {
                        upsert_instruction(&tx, *id, instr)?;
                    }
                }
            }
            drop(stmt);
            tx.commit()?;
            Ok(n)
        })
    }

    /// Batch-delete TUs — any status, including `translating`/`approved`/
    /// `exported` (the user deletes 不需要/识别错误的段 outright). Each TU's
    /// translation row cascades via FK. A paragraph referenced by a deleted TU
    /// is removed only if **no remaining TU** still references it (a paragraph
    /// may belong to several TUs; shared paragraphs are kept). All inside one
    /// transaction. Returns the number of TUs actually deleted.
    pub fn delete_tus(&self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.db.write(|conn| {
            let tx = conn.transaction()?;
            // Collect every paragraph the deleted TUs referenced.
            let mut paras: Vec<String> = Vec::new();
            {
                let mut stmt = tx.prepare("SELECT paragraph_ids FROM tus WHERE id = ?1")?;
                for id in ids {
                    let json: Option<String> = stmt.query_row([id], |r| r.get(0)).optional()?;
                    if let Some(json) = json {
                        if let Ok(list) = serde_json::from_str::<Vec<String>>(&json) {
                            paras.extend(list);
                        }
                    }
                }
            }
            // Drop the TU rows (translations rows cascade via FK).
            let mut deleted = 0usize;
            {
                let mut stmt = tx.prepare("DELETE FROM tus WHERE id = ?1")?;
                for id in ids {
                    if stmt.execute([id])? == 1 {
                        deleted += 1;
                    }
                }
            }
            // A paragraph survives if any remaining TU still references it
            // (paragraph_ids is a JSON array of UUID strings, so a quoted-uuid
            // containment match is exact — UUIDs contain no `%`/`_`).
            {
                let mut refs = tx.prepare("SELECT COUNT(*) FROM tus WHERE paragraph_ids LIKE ?1")?;
                let mut del = tx.prepare("DELETE FROM paragraphs WHERE id = ?1")?;
                for pid in paras {
                    let pattern = format!("%\"{pid}\"%");
                    let n: i64 = refs.query_row([&pattern], |r| r.get(0))?;
                    if n == 0 {
                        del.execute([&pid])?;
                    }
                }
            }
            tx.commit()?;
            Ok(deleted)
        })
    }

    // ----- pipeline queries -------------------------------------------------

    /// TU ids eligible for translation in the given chapters, ordered by
    /// `(chapter.ord, tu.ord)` — the global priority order.
    pub fn next_eligible_tus(&self, chapter_ids: &[i64], limit: usize) -> Result<Vec<i64>> {
        if chapter_ids.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        self.db.read(|c| {
            // Explicitly-numbered placeholders (?1..?n for chapters, ?{n+1} for
            // the limit): mixing bare `?` with `?1` makes rusqlite assign the
            // limit value to `IN (?)` and the first chapter id to `LIMIT ?1`.
            let n = chapter_ids.len();
            let ph = (1..=n).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT t.id FROM tus t JOIN chapters c ON t.chapter_id = c.id
                 WHERE t.status IN ('pending','queued') AND t.chapter_id IN ({ph})
                 ORDER BY c.ord, t.ord LIMIT ?{}",
                n + 1
            );
            let mut stmt = c.prepare(&sql)?;
            let mut params: Vec<&dyn rusqlite::ToSql> =
                chapter_ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let limit_i64 = limit as i64;
            params.push(&limit_i64);
            let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, i64>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    /// Persist the user's edited 原文 for a TU. Stored in `source_override`, so
    /// the underlying paragraphs stay untouched and re-segmentation is unaffected;
    /// [`Self::tu_source`] and [`Self::list_tus_with_translations`] both prefer
    /// the override. An empty/blank source clears the override.
    pub fn set_tu_source(&self, tu_id: i64, source: &str) -> Result<()> {
        self.db.write(|c| {
            c.execute(
                "UPDATE tus SET source_override = NULLIF(?1, '') WHERE id = ?2",
                rusqlite::params![source, tu_id],
            )?;
            Ok(())
        })
    }

    /// Concatenated source text of a TU — its paragraphs, in order, joined by
    /// `\n` — unless the user set a `source_override`, which wins verbatim.
    pub fn tu_source(&self, tu_id: i64) -> Result<String> {
        self.db.read(|c| {
            let (ids_json, override_text): (Option<String>, Option<String>) = c
                .query_row(
                    "SELECT paragraph_ids, source_override FROM tus WHERE id = ?1",
                    [tu_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .unwrap_or((None, None));
            if let Some(s) = override_text.filter(|s| !s.trim().is_empty()) {
                return Ok(s);
            }
            let Some(ids_json) = ids_json else { return Ok(String::new()) };
            let ids: Vec<String> = serde_json::from_str(&ids_json).unwrap_or_default();
            if ids.is_empty() {
                return Ok(String::new());
            }
            let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("SELECT text FROM paragraphs WHERE id IN ({ph}) ORDER BY ord");
            let mut stmt = c.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::ToSql> =
                ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(params.as_slice(), |r| r.get::<_, String>(0))?;
            let texts: Vec<String> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(texts.join("\n"))
        })
    }

    /// Translation-memory lookup: an approved/exported TU elsewhere in the same
    /// project with the same normalized source hash.
    pub fn find_memory_hit(&self, source_hash: &str) -> Result<Option<(i64, String)>> {
        self.db.read(|c| {
            c.query_row(
                "SELECT t.tu_id, t.final_text FROM translations t
                 JOIN tus tu ON t.tu_id = tu.id
                 JOIN chapters c ON tu.chapter_id = c.id
                 WHERE t.source_hash = ?1 AND t.final_text IS NOT NULL
                   AND tu.status IN ('approved','exported')
                 ORDER BY c.ord, tu.ord LIMIT 1",
                [source_hash],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(Into::into)
        })
    }

    /// The previous approved TU's final text in the same chapter (style/naming
    /// context for the current TU).
    pub fn prev_approved_context(&self, chapter_id: i64, tu_id: i64) -> Result<Option<String>> {
        self.db.read(|c| {
            c.query_row(
                "SELECT t.final_text FROM translations t
                 JOIN tus tu ON t.tu_id = tu.id
                 WHERE tu.chapter_id = ?1
                   AND tu.ord < (SELECT ord FROM tus WHERE id = ?2)
                   AND tu.status IN ('approved','exported') AND t.final_text IS NOT NULL
                 ORDER BY tu.ord DESC LIMIT 1",
                rusqlite::params![chapter_id, tu_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
        })
    }

    /// Per-status TU counts (for the task panel).
    pub fn counts_by_status(&self) -> Result<Vec<(TuStatus, i64)>> {
        self.db.read(|c| {
            let mut stmt = c.prepare("SELECT status, COUNT(*) FROM tus GROUP BY status")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, TuStatus>(0)?, r.get::<_, i64>(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    /// TUs of a chapter joined with their translation rows, for the review
    /// cards. `source` is the effective 原文 — the user's `source_override` if
    /// set (non-blank), else the TU's paragraphs concatenated in order.
    /// `matched_names` are the *enabled* small-glossary entries `source` hits
    /// (what prompt injection applied), computed here at query time from the
    /// same matcher data — one compilation shared across all the TUs.
    pub fn list_tus_with_translations(&self, chapter_id: i64) -> Result<Vec<TuWithTranslation>> {
        let glossary = crate::pipeline::prompt::GlossaryMatcher::build(&self.matcher_entries()?)?;
        self.db.read(|c| {
            // Load this chapter's paragraph text once, keyed by id, so effective
            // sources resolve without one query per TU.
            let mut para_text: HashMap<String, String> = HashMap::new();
            {
                let mut stmt = c.prepare("SELECT id, text FROM paragraphs WHERE chapter_id = ?1")?;
                let rows = stmt.query_map([chapter_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                for row in rows {
                    let (id, text) = row?;
                    para_text.insert(id, text);
                }
            }

            let mut stmt = c.prepare(
                "SELECT t.id, t.ord, t.budget, t.status, t.paragraph_ids, t.source_override,
                        tr.status, tr.final_text, tr.llm_text, tr.instruction, tr.error, tr.source_hash
                 FROM tus t LEFT JOIN translations tr ON tr.tu_id = t.id
                 WHERE t.chapter_id = ?1 ORDER BY t.ord",
            )?;
            let rows = stmt.query_map([chapter_id], |r| {
                let ids_json: String = r.get(4)?;
                let paragraph_ids: Vec<String> = serde_json::from_str(&ids_json).unwrap_or_default();
                let override_text: Option<String> = r.get(5)?;
                let source = match override_text.filter(|s| !s.trim().is_empty()) {
                    Some(s) => s,
                    None => {
                        let mut parts = Vec::with_capacity(paragraph_ids.len());
                        for pid in &paragraph_ids {
                            if let Some(t) = para_text.get(pid) {
                                parts.push(t.as_str());
                            }
                        }
                        parts.join("\n")
                    }
                };
                let matched_names =
                    glossary.as_ref().map_or_else(Vec::new, |g| g.matched_names(&source));
                Ok(TuWithTranslation {
                    id: r.get(0)?,
                    ord: r.get(1)?,
                    budget: r.get(2)?,
                    status: r.get(3)?,
                    translation_status: r.get(6)?,
                    final_text: r.get(7)?,
                    llm_text: r.get(8)?,
                    instruction: r.get(9)?,
                    error: r.get(10)?,
                    source_hash: r.get(11)?,
                    source,
                    matched_names,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    /// The translation row for a TU, if any.
    pub fn get_translation(&self, tu_id: i64) -> Result<Option<Translation>> {
        self.db.read(|c| get_translation_row(c, tu_id).map_err(Into::into))
    }

    // ----- extracted proper-noun candidates -------------------------------

    /// Insert a candidate unless one with the same `japanese` already exists.
    /// `category` is the tag proposed by the LLM (appended to the empty default
    /// tags array). Returns the new row id, or `None` if it was a duplicate.
    pub fn insert_extracted(
        &self,
        japanese: &str,
        candidate_chinese: Option<&str>,
        category: Option<&str>,
        notes: Option<&str>,
    ) -> Result<Option<i64>> {
        self.db.write(|c| {
            let changed = c.execute(
                "INSERT INTO extracted_names (japanese, candidate_chinese, status, tags, notes)
                 SELECT ?1, ?2, 'new', ?3, ?4
                 WHERE NOT EXISTS (SELECT 1 FROM extracted_names WHERE japanese = ?1)",
                rusqlite::params![
                    japanese,
                    candidate_chinese,
                    serde_json::to_string(&tags_of(category)).unwrap_or_else(|_| "[]".into()),
                    notes,
                ],
            )?;
            Ok((changed > 0).then(|| c.last_insert_rowid()))
        })
    }

    /// List candidates, optionally filtered by status, ordered by id.
    pub fn list_extracted_names(&self, status: Option<ExtractedNameStatus>) -> Result<Vec<ExtractedName>> {
        self.db.read(|c| {
            let mut v = Vec::new();
            match status {
                Some(s) => {
                    let mut stmt = c.prepare(
                        "SELECT id, japanese, matched_name_id, candidate_chinese, status, tags, notes
                         FROM extracted_names WHERE status = ?1 ORDER BY id",
                    )?;
                    for row in stmt.query_map([s], row_to_extracted)? {
                        v.push(row?);
                    }
                }
                None => {
                    let mut stmt = c.prepare(
                        "SELECT id, japanese, matched_name_id, candidate_chinese, status, tags, notes
                         FROM extracted_names ORDER BY id",
                    )?;
                    for row in stmt.query_map([], row_to_extracted)? {
                        v.push(row?);
                    }
                }
            }
            Ok(v)
        })
    }

    /// Fetch one candidate by id.
    pub fn get_extracted(&self, id: i64) -> Result<Option<ExtractedName>> {
        self.db.read(|c| {
            c.query_row(
                "SELECT id, japanese, matched_name_id, candidate_chinese, status, tags, notes
                 FROM extracted_names WHERE id = ?1",
                [id],
                row_to_extracted,
            )
            .optional()
            .map_err(Into::into)
        })
    }

    /// Set a candidate's review status (and optionally link it to a glossary id).
    pub fn set_extracted_status(
        &self,
        id: i64,
        status: ExtractedNameStatus,
        matched_name_id: Option<i64>,
    ) -> Result<()> {
        self.db.write(|c| {
            c.execute(
                "UPDATE extracted_names
                 SET status = ?1, matched_name_id = COALESCE(?2, matched_name_id) WHERE id = ?3",
                rusqlite::params![status, matched_name_id, id],
            )?;
            Ok(())
        })
    }

    /// Edit a candidate's proposed Chinese rendering.
    pub fn update_extracted_chinese(&self, id: i64, chinese: &str) -> Result<()> {
        self.db.write(|c| {
            c.execute(
                "UPDATE extracted_names SET candidate_chinese = ?1 WHERE id = ?2",
                rusqlite::params![chinese, id],
            )?;
            Ok(())
        })
    }

    /// Replace a candidate's category tags (JSON array).
    pub fn set_extracted_tags(&self, id: i64, tags: &[String]) -> Result<()> {
        self.db.write(|c| {
            c.execute(
                "UPDATE extracted_names SET tags = ?1 WHERE id = ?2",
                rusqlite::params![serde_json::to_string(tags).unwrap_or_else(|_| "[]".into()), id],
            )?;
            Ok(())
        })
    }

    // ----- project small glossary ------------------------------------------

    /// Insert a glossary entry, or refresh an existing one matched by `japanese`
    /// (value fields overwrite; timestamps update). `name_global_id` records
    /// provenance in the global big glossary. Returns the row id.
    pub fn insert_glossary_entry(
        &self,
        name_global_id: Option<i64>,
        japanese: &str,
        chinese: Option<&str>,
        english: Option<&str>,
        category: Option<&str>,
        tags: &[String],
        notes: Option<&str>,
    ) -> Result<i64> {
        let now = now_iso8601();
        self.db.write(|c| {
            let tx = c.transaction()?;
            tx.execute(
                "INSERT INTO glossary_entries
                   (name_global_id, japanese, chinese, english, category, tags, notes, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT(japanese) DO UPDATE SET
                     name_global_id = COALESCE(excluded.name_global_id, glossary_entries.name_global_id),
                     chinese   = excluded.chinese,
                     english   = excluded.english,
                     category  = excluded.category,
                     tags      = excluded.tags,
                     notes     = excluded.notes,
                     updated_at = excluded.updated_at",
                rusqlite::params![
                    name_global_id,
                    japanese,
                    chinese,
                    english,
                    category,
                    serde_json::to_string(tags).unwrap_or_else(|_| "[]".into()),
                    notes,
                    now,
                ],
            )?;
            let id: i64 = tx.query_row(
                "SELECT id FROM glossary_entries WHERE japanese = ?1",
                [japanese],
                |r| r.get(0),
            )?;
            tx.commit()?;
            Ok(id)
        })
    }

    /// Fully edit an entry's value fields (tags replace wholesale; the
    /// `enabled` flag is untouched — use [`Self::set_entry_enabled`]).
    pub fn update_glossary_entry(
        &self,
        id: i64,
        japanese: &str,
        chinese: Option<&str>,
        english: Option<&str>,
        category: Option<&str>,
        tags: &[String],
        notes: Option<&str>,
    ) -> Result<()> {
        self.db.write(|c| {
            c.execute(
                "UPDATE glossary_entries
                 SET japanese = ?1, chinese = ?2, english = ?3, category = ?4,
                     tags = ?5, notes = ?6, updated_at = ?7
                 WHERE id = ?8",
                rusqlite::params![
                    japanese,
                    chinese,
                    english,
                    category,
                    serde_json::to_string(tags).unwrap_or_else(|_| "[]".into()),
                    notes,
                    now_iso8601(),
                    id,
                ],
            )?;
            Ok(())
        })
    }

    /// Toggle an entry's translation-injection flag.
    pub fn set_entry_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.db.write(|c| {
            c.execute(
                "UPDATE glossary_entries SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![enabled as i64, now_iso8601(), id],
            )?;
            Ok(())
        })
    }

    /// Replace an entry's tag array (JSON).
    pub fn set_entry_tags(&self, id: i64, tags: &[String]) -> Result<()> {
        self.db.write(|c| {
            c.execute(
                "UPDATE glossary_entries SET tags = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![serde_json::to_string(tags).unwrap_or_else(|_| "[]".into()), now_iso8601(), id],
            )?;
            Ok(())
        })
    }

    /// Delete an entry from the project small glossary.
    pub fn delete_glossary_entry(&self, id: i64) -> Result<()> {
        self.db.write(|c| {
            c.execute("DELETE FROM glossary_entries WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Batch-delete entries from the project small glossary, in one statement.
    /// Returns the number of rows actually deleted.
    pub fn delete_glossary_entries(&self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.db.write(|c| {
            let ph = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
            let sql = format!("DELETE FROM glossary_entries WHERE id IN ({ph})");
            Ok(c.execute(&sql, rusqlite::params_from_iter(ids.iter()))?)
        })
    }

    /// Clear the `name_global_id` provenance pointer on small-glossary entries
    /// whose global entry is being deleted, so no dangling cross-file reference
    /// survives. The small-glossary data itself is kept (it is self-contained).
    /// Returns the number of entries touched.
    pub fn clear_global_provenance(&self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.db.write(|c| {
            let ph = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
            let sql =
                format!("UPDATE glossary_entries SET name_global_id = NULL WHERE name_global_id IN ({ph})");
            Ok(c.execute(&sql, rusqlite::params_from_iter(ids.iter()))?)
        })
    }

    /// List the project small glossary, optionally narrowed by a free-text
    /// search across japanese/chinese/english/tags, ordered by id.
    pub fn list_glossary_entries(&self, q: Option<&str>) -> Result<Vec<GlossaryEntry>> {
        let (sql, param) = match q.map(str::trim).filter(|s| !s.is_empty()) {
            Some(pat) => (
                "SELECT id, name_global_id, japanese, chinese, english, category, tags, enabled,
                        notes, created_at, updated_at
                 FROM glossary_entries
                 WHERE japanese LIKE ?1 OR chinese LIKE ?1 OR english LIKE ?1
                    OR tags LIKE ?1
                 ORDER BY id",
                format!("%{pat}%"),
            ),
            None => (
                "SELECT id, name_global_id, japanese, chinese, english, category, tags, enabled,
                        notes, created_at, updated_at
                 FROM glossary_entries ORDER BY id",
                String::new(),
            ),
        };
        self.db.read(move |c| {
            let mut stmt = c.prepare(&sql)?;
            let rows = if param.is_empty() {
                stmt.query_map([], row_to_glossary_entry)?
            } else {
                stmt.query_map([param.as_str()], row_to_glossary_entry)?
            };
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    /// The enabled entries — exactly what translation prompt injection reads
    /// (japanese feeds the matcher; the global big glossary is never injected
    /// directly).
    pub fn matcher_entries(&self) -> Result<Vec<GlossaryEntry>> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, name_global_id, japanese, chinese, english, category, tags, enabled,
                        notes, created_at, updated_at
                 FROM glossary_entries WHERE enabled = 1 ORDER BY id",
            )?;
            let rows = stmt.query_map([], row_to_glossary_entry)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    // ----- deterministic 译文导出 -------------------------------------------

    /// Export the project's translated text: a 汉化 .txt (per chapter `# {title}`
    /// then the non-empty `final_text`s in TU order) plus a 译文.csv (one row per
    /// translated TU). Deterministic: chapters by `ord`, TUs by `ord`, only TUs
    /// with a non-empty `final_text`. Both paths are recorded in `exports`.
    pub fn export_translations(&self, dest_dir: &Path) -> Result<TranslationExport> {
        let rows = self.list_export_rows()?;
        std::fs::create_dir_all(dest_dir)?;
        let txt_path = dest_dir.join("汉化.txt");
        let csv_path = dest_dir.join("译文.csv");

        let mut txt = String::new();
        for (_, title, items) in &rows {
            txt.push_str(&format!("# {title}\n"));
            for (_, _, final_text) in items {
                txt.push_str(final_text);
                txt.push('\n');
            }
            txt.push('\n');
        }
        std::fs::write(&txt_path, txt)?;

        {
            let mut wtr = csv::Writer::from_path(&csv_path)?;
            wtr.write_record(["章号", "章节标题", "序号", "原文", "译文", "状态"])?;
            for (chapter_ord, title, items) in &rows {
                for (tu_ord, source, final_text) in items {
                    wtr.write_record([
                        chapter_ord.to_string(),
                        title.clone(),
                        tu_ord.to_string(),
                        source.clone(),
                        final_text.clone(),
                        "approved".into(),
                    ])?;
                }
            }
            wtr.flush()?;
        }

        let now = now_iso8601();
        self.db.write(|c| {
            c.execute(
                "INSERT INTO exports (path, created_at) VALUES (?1, ?2)",
                rusqlite::params![txt_path.to_string_lossy(), now],
            )?;
            c.execute(
                "INSERT INTO exports (path, created_at) VALUES (?1, ?2)",
                rusqlite::params![csv_path.to_string_lossy(), now],
            )?;
            Ok(())
        })?;

        let tus: usize = rows.iter().map(|(_, _, items)| items.len()).sum();
        Ok(TranslationExport {
            txt_path: txt_path.to_string_lossy().into_owned(),
            csv_path: csv_path.to_string_lossy().into_owned(),
            tus,
        })
    }

    /// Per-chapter translated TUs in deterministic order — one entry per chapter
    /// (chapter `ord`, title), each a list of `(tu_ord, source, final_text)`
    /// ordered by TU `ord`, only non-empty `final_text`s. Uses the same effective
    /// source resolution as [`Self::list_tus_with_translations`] (override wins).
    fn list_export_rows(&self) -> Result<Vec<(i64, String, Vec<(i64, String, String)>)>> {
        self.db.read(|c| {
            let mut chapters = Vec::new();
            {
                let mut stmt = c.prepare("SELECT id, title, ord FROM chapters ORDER BY ord")?;
                let rows = stmt.query_map([], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
                })?;
                for row in rows {
                    chapters.push(row?);
                }
            }
            let mut out = Vec::with_capacity(chapters.len());
            for (chapter_id, title, chapter_ord) in chapters {
                let mut para_text: HashMap<String, String> = HashMap::new();
                {
                    let mut stmt = c.prepare("SELECT id, text FROM paragraphs WHERE chapter_id = ?1")?;
                    let rows = stmt.query_map([chapter_id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?;
                    for row in rows {
                        let (id, text) = row?;
                        para_text.insert(id, text);
                    }
                }
                let mut items = Vec::new();
                {
                    let mut stmt = c.prepare(
                        "SELECT t.ord, t.paragraph_ids, t.source_override, tr.final_text
                         FROM tus t LEFT JOIN translations tr ON tr.tu_id = t.id
                         WHERE t.chapter_id = ?1 AND tr.final_text IS NOT NULL AND tr.final_text <> ''
                         ORDER BY t.ord",
                    )?;
                    let rows = stmt.query_map([chapter_id], |r| {
                        let ids_json: String = r.get(1)?;
                        let paragraph_ids: Vec<String> = serde_json::from_str(&ids_json).unwrap_or_default();
                        let override_text: Option<String> = r.get(2)?;
                        let source = match override_text.filter(|s| !s.trim().is_empty()) {
                            Some(s) => s,
                            None => {
                                let mut parts = Vec::with_capacity(paragraph_ids.len());
                                for pid in &paragraph_ids {
                                    if let Some(t) = para_text.get(pid) {
                                        parts.push(t.as_str());
                                    }
                                }
                                parts.join("\n")
                            }
                        };
                        Ok((r.get::<_, i64>(0)?, source, r.get::<_, String>(3)?))
                    })?;
                    for row in rows {
                        items.push(row?);
                    }
                }
                out.push((chapter_ord, title, items));
            }
            Ok(out)
        })
    }

    /// (Re-)segment the project: clean paragraph text, drop artifact-only
    /// paragraphs, detect chapters, reassign paragraphs to them, and rebuild
    /// TUs — all in one transaction. Refuses to run once any translation exists
    /// (preservation of approved work lands with the pipeline milestone).
    pub fn segment(&self, budget: usize, fallback_title: &str, recognizer: &ChapterRecognizer) -> Result<SegmentOutcome> {
        self.db.write(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

            let translations: i64 = tx.query_row("SELECT COUNT(*) FROM translations", [], |r| r.get(0))?;
            if translations > 0 {
                return Err(Error::InvalidInput {
                    detail: "cannot re-segment after translation has started".into(),
                });
            }

            // Load all paragraphs in reading order.
            let loaded: Vec<(String, String)> = {
                let mut stmt = tx.prepare(
                    "SELECT p.id, p.text FROM paragraphs p JOIN chapters c ON p.chapter_id = c.id
                     ORDER BY c.ord, p.ord",
                )?;
                let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            // Clean; drop paragraphs that were nothing but artifacts.
            let mut survivors: Vec<(String, String)> = Vec::new();
            for (id, text) in loaded {
                let cleaned = clean_text(&text);
                if cleaned.is_empty() {
                    tx.execute("DELETE FROM paragraphs WHERE id = ?1", [&id])?;
                } else {
                    survivors.push((id, cleaned));
                }
            }
            tx.execute("DELETE FROM tus", [])?;
            if survivors.is_empty() {
                tx.commit()?;
                return Ok(SegmentOutcome { chapters: 0, tus: 0 });
            }

            let texts: Vec<&str> = survivors.iter().map(|(_, t)| t.as_str()).collect();
            let cuts = recognizer.detect(&texts, fallback_title);

            let mut new_chapter_ids: Vec<i64> = Vec::new();
            let mut total_tus = 0usize;
            for (ci, cut) in cuts.iter().enumerate() {
                let end = cuts.get(ci + 1).map(|n| n.start).unwrap_or(survivors.len());
                tx.execute(
                    "INSERT INTO chapters (title, ord, status) VALUES (?1, ?2, ?3)",
                    rusqlite::params![cut.title, ci as i64, ChapterStatus::Pending],
                )?;
                let chapter_id = tx.last_insert_rowid();
                new_chapter_ids.push(chapter_id);

                let slice = &survivors[cut.start..end];
                {
                    let mut up =
                        tx.prepare("UPDATE paragraphs SET chapter_id=?1, ord=?2, text=?3 WHERE id=?4")?;
                    for (ord, (id, text)) in slice.iter().enumerate() {
                        up.execute(rusqlite::params![chapter_id, ord as i64, text, id])?;
                    }
                }

                let paras: Vec<(Uuid, usize)> = slice
                    .iter()
                    .map(|(id, text)| {
                        (Uuid::parse_str(id).unwrap_or_else(|_| Uuid::nil()), text.chars().count())
                    })
                    .collect();
                {
                    let mut ins = tx.prepare(
                        "INSERT INTO tus (chapter_id, paragraph_ids, ord, budget, status)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                    )?;
                    for (ord, plan) in aggregate(&paras, budget).iter().enumerate() {
                        let ids: Vec<String> = plan.paragraph_ids.iter().map(|u| u.to_string()).collect();
                        ins.execute(rusqlite::params![
                            chapter_id,
                            serde_json::to_string(&ids)?,
                            ord as i64,
                            plan.char_len as i64,
                            TuStatus::Pending,
                        ])?;
                        total_tus += 1;
                    }
                }
            }

            // Remove the now-empty old chapters (their paragraphs were reassigned,
            // so ON DELETE CASCADE removes nothing needed).
            let placeholders = new_chapter_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("DELETE FROM chapters WHERE id NOT IN ({placeholders})");
            let params: Vec<&dyn rusqlite::ToSql> =
                new_chapter_ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            tx.execute(&sql, params.as_slice())?;

            tx.commit()?;
            Ok(SegmentOutcome { chapters: new_chapter_ids.len(), tus: total_tus })
        })
    }
}

fn row_to_paragraph(r: &rusqlite::Row<'_>) -> rusqlite::Result<Paragraph> {
    let meta_str: Option<String> = r.get(7)?;
    let ocr_meta = meta_str.and_then(|s| serde_json::from_str(&s).ok());
    Ok(Paragraph {
        id: r.get(0)?,
        chapter_id: r.get(1)?,
        ord: r.get(2)?,
        text: r.get(3)?,
        page_num: r.get(4)?,
        page_score: r.get(5)?,
        ocr_status: r.get(6)?,
        ocr_meta,
        source_file: r.get(8)?,
    })
}

fn row_to_tu(r: &rusqlite::Row<'_>) -> rusqlite::Result<Tu> {
    let ids_json: String = r.get(2)?;
    let paragraph_ids: Vec<String> = serde_json::from_str(&ids_json).unwrap_or_default();
    Ok(Tu {
        id: r.get(0)?,
        chapter_id: r.get(1)?,
        paragraph_ids,
        ord: r.get(3)?,
        budget: r.get(4)?,
        status: r.get(5)?,
    })
}

/// A single non-empty category tag (or an empty vec when `category` is blank) —
/// how the LLM-proposed category becomes the initial `tags` array on a new
/// extracted candidate.
fn tags_of(category: Option<&str>) -> Vec<String> {
    category
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .into_iter()
        .collect()
}

fn row_to_extracted(r: &rusqlite::Row<'_>) -> rusqlite::Result<ExtractedName> {
    let tags_json: String = r.get(5)?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    Ok(ExtractedName {
        id: r.get(0)?,
        japanese: r.get(1)?,
        matched_name_id: r.get(2)?,
        candidate_chinese: r.get(3)?,
        status: r.get(4)?,
        tags,
        notes: r.get(6)?,
    })
}

/// Map a `glossary_entries` row (column order: id, name_global_id, japanese,
/// chinese, english, category, tags, enabled, notes, created_at, updated_at)
/// to a [`GlossaryEntry`].
fn row_to_glossary_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<GlossaryEntry> {
    let tags_json: String = r.get(6)?;
    Ok(GlossaryEntry {
        id: r.get(0)?,
        name_global_id: r.get(1)?,
        japanese: r.get(2)?,
        chinese: r.get(3)?,
        english: r.get(4)?,
        category: r.get(5)?,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        enabled: r.get::<_, i64>(7)? != 0,
        notes: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

fn row_to_translation(r: &rusqlite::Row<'_>) -> rusqlite::Result<Translation> {
    let attempts_json: String = r.get(6)?;
    let attempts: Vec<String> = serde_json::from_str(&attempts_json).unwrap_or_default();
    Ok(Translation {
        id: r.get(0)?,
        tu_id: r.get(1)?,
        status: r.get(2)?,
        source_hash: r.get(3)?,
        llm_text: r.get(4)?,
        final_text: r.get(5)?,
        attempts,
        instruction: r.get(7)?,
        error: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
    })
}

/// Fetch one translation row by TU id (column order must match
/// [`row_to_translation`]).
fn get_translation_row(
    conn: &rusqlite::Connection,
    tu_id: i64,
) -> rusqlite::Result<Option<Translation>> {
    let mut stmt = conn.prepare(
        "SELECT id, tu_id, status, source_hash, llm_text, final_text, attempts, instruction,
                error, created_at, updated_at
         FROM translations WHERE tu_id = ?1",
    )?;
    let mut rows = stmt.query_map([tu_id], row_to_translation)?;
    rows.next().transpose()
}

/// Insert or update the translation row for a TU. On update, the previous draft
/// is pushed into `attempts`. `memory=true` marks the row `memory_hit` (no LLM
/// output; `llm_text` stays NULL) and uses `draft` as the pre-filled final text.
fn upsert_translation(
    conn: &rusqlite::Connection,
    tu_id: i64,
    draft: &str,
    source_hash: &str,
    memory: bool,
) -> rusqlite::Result<()> {
    let status = if memory { "memory_hit" } else { "draft" };
    let now = now_iso8601();
    let llm_text: Option<&str> = if memory { None } else { Some(draft) };
    if let Some(mut old) = get_translation_row(conn, tu_id)? {
        if let Some(prev) = old.llm_text.take().or_else(|| old.final_text.clone()) {
            old.attempts.push(prev);
        }
        conn.execute(
            "UPDATE translations
             SET status = ?1, source_hash = ?2, llm_text = ?3, final_text = ?4, attempts = ?5,
                 error = NULL, updated_at = ?6
             WHERE tu_id = ?7",
            rusqlite::params![
                status,
                source_hash,
                llm_text,
                draft,
                serde_json::to_string(&old.attempts).unwrap_or_else(|_| "[]".into()),
                now,
                tu_id,
            ],
        )?;
    } else {
        conn.execute(
            "INSERT INTO translations (tu_id, status, source_hash, llm_text, final_text, attempts, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, '[]', ?6, ?6)",
            rusqlite::params![tu_id, status, source_hash, llm_text, draft, now],
        )?;
    }
    Ok(())
}

/// Insert or update the failure error on a TU's translation row (creating the
/// row if the failure preceded any draft).
fn upsert_error(conn: &rusqlite::Connection, tu_id: i64, error: &str) -> rusqlite::Result<()> {
    let now = now_iso8601();
    let changed = conn.execute(
        "UPDATE translations SET status = 'failed', error = ?1, updated_at = ?2 WHERE tu_id = ?3",
        rusqlite::params![error, now, tu_id],
    )?;
    if changed == 0 {
        conn.execute(
            "INSERT INTO translations (tu_id, status, error, attempts, created_at, updated_at)
             VALUES (?1, 'failed', ?2, '[]', ?3, ?3)",
            rusqlite::params![tu_id, error, now],
        )?;
    }
    Ok(())
}

/// Insert or update a per-item instruction on a TU's translation row (creating
/// the row if absent).
fn upsert_instruction(conn: &rusqlite::Connection, tu_id: i64, instruction: &str) -> rusqlite::Result<()> {
    let now = now_iso8601();
    let changed = conn.execute(
        "UPDATE translations SET instruction = ?1, updated_at = ?2 WHERE tu_id = ?3",
        rusqlite::params![instruction, now, tu_id],
    )?;
    if changed == 0 {
        conn.execute(
            "INSERT INTO translations (tu_id, status, instruction, attempts, created_at, updated_at)
             VALUES (?1, 'draft', ?2, '[]', ?3, ?3)",
            rusqlite::params![tu_id, instruction, now],
        )?;
    }
    Ok(())
}

/// Persist a human-edited `final_text` on a TU's translation row (creating the
/// row if absent) and clear any recorded failure.
fn upsert_final_text(conn: &rusqlite::Connection, tu_id: i64, final_text: &str) -> rusqlite::Result<()> {
    let now = now_iso8601();
    let changed = conn.execute(
        "UPDATE translations SET final_text = ?1, status = 'draft', error = NULL, updated_at = ?2
         WHERE tu_id = ?3",
        rusqlite::params![final_text, now, tu_id],
    )?;
    if changed == 0 {
        conn.execute(
            "INSERT INTO translations (tu_id, status, final_text, attempts, created_at, updated_at)
             VALUES (?1, 'draft', ?2, '[]', ?3, ?3)",
            rusqlite::params![tu_id, final_text, now],
        )?;
    }
    Ok(())
}

fn parse_i64(s: Option<&str>, default: i64, min: i64, max: i64) -> i64 {
    s.and_then(|v| v.parse::<i64>().ok())
        .map(|v| v.clamp(min, max))
        .unwrap_or(default)
}

fn parse_bool(s: Option<&str>, default: bool) -> bool {
    match s {
        Some(v) => v.eq_ignore_ascii_case("true"),
        None => default,
    }
}
