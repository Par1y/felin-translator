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
    Chapter, ChapterStatus, ExtractedName, ExtractedNameStatus, IngestedParagraph, Paragraph, Tu,
    TuStatus,
};
use rusqlite::OptionalExtension;
use std::path::Path;
use uuid::Uuid;

/// Result of a segmentation pass.
#[derive(Debug, Clone, Copy)]
pub struct SegmentOutcome {
    pub chapters: usize,
    pub tus: usize,
}

/// Ordered migration set for the project DB. Append new migrations here.
pub const PROJECT_MIGRATIONS: &[Migration] =
    &[Migration { version: 1, sql: include_str!("migrations/project/0001_init.sql") }];

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

    // ----- extracted proper-noun candidates -------------------------------

    /// Insert a candidate unless one with the same `japanese` already exists.
    /// Returns the new row id, or `None` if it was a duplicate.
    pub fn insert_extracted(
        &self,
        japanese: &str,
        candidate_chinese: Option<&str>,
        notes: Option<&str>,
    ) -> Result<Option<i64>> {
        self.db.write(|c| {
            let changed = c.execute(
                "INSERT INTO extracted_names (japanese, candidate_chinese, status, notes)
                 SELECT ?1, ?2, 'new', ?3
                 WHERE NOT EXISTS (SELECT 1 FROM extracted_names WHERE japanese = ?1)",
                rusqlite::params![japanese, candidate_chinese, notes],
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
                        "SELECT id, japanese, matched_name_id, candidate_chinese, status, notes
                         FROM extracted_names WHERE status = ?1 ORDER BY id",
                    )?;
                    for row in stmt.query_map([s], row_to_extracted)? {
                        v.push(row?);
                    }
                }
                None => {
                    let mut stmt = c.prepare(
                        "SELECT id, japanese, matched_name_id, candidate_chinese, status, notes
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
                "SELECT id, japanese, matched_name_id, candidate_chinese, status, notes
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

fn row_to_extracted(r: &rusqlite::Row<'_>) -> rusqlite::Result<ExtractedName> {
    Ok(ExtractedName {
        id: r.get(0)?,
        japanese: r.get(1)?,
        matched_name_id: r.get(2)?,
        candidate_chinese: r.get(3)?,
        status: r.get(4)?,
        notes: r.get(5)?,
    })
}
