//! Global glossary database (`glossary.db`) — the single source of truth for
//! proper nouns, shared across all projects.

use crate::error::Result;
use crate::storage::db::{Db, DbTuning};
use crate::storage::migrations::Migration;
use crate::types::{GlossaryName, NameStatus};
use crate::util::now_iso8601;
use rusqlite::OptionalExtension;
use std::path::Path;

/// Ordered migration set for the global DB. Append new migrations here.
pub const GLOBAL_MIGRATIONS: &[Migration] =
    &[Migration { version: 1, sql: include_str!("migrations/global/0001_init.sql") }];

/// Typed wrapper over the global glossary database.
#[derive(Debug)]
pub struct GlobalDb {
    db: Db,
}

impl GlobalDb {
    /// Open (creating + migrating if needed) the global DB at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with(path, DbTuning::default())
    }

    /// Open with explicit connection tuning.
    pub fn open_with(path: &Path, tuning: DbTuning) -> Result<Self> {
        Ok(Self { db: Db::open(path, GLOBAL_MIGRATIONS, false, tuning)? })
    }

    /// Access the underlying handle for queries not yet wrapped here.
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Insert a name, or update an existing one matched by `japanese`
    /// (non-null `chinese` overwrites; `created_at` is preserved). Returns the
    /// row id.
    pub fn upsert_name(
        &self,
        japanese: &str,
        chinese: Option<&str>,
        source: &str,
        status: NameStatus,
    ) -> Result<i64> {
        let now = now_iso8601();
        self.db.write(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO names (japanese, chinese, source, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(japanese) DO UPDATE SET
                     chinese    = COALESCE(excluded.chinese, names.chinese),
                     updated_at = excluded.updated_at",
                rusqlite::params![japanese, chinese, source, status, now],
            )?;
            let id: i64 =
                tx.query_row("SELECT id FROM names WHERE japanese = ?1", [japanese], |r| r.get(0))?;
            tx.commit()?;
            Ok(id)
        })
    }

    /// Look up the Chinese rendering for a Japanese form, if present.
    pub fn chinese_for(&self, japanese: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        self.db.read(|c| {
            c.query_row("SELECT chinese FROM names WHERE japanese = ?1", [japanese], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()
            .map(Option::flatten)
            .map_err(Into::into)
        })
    }

    /// Total number of glossary entries.
    pub fn count_names(&self) -> Result<i64> {
        self.db.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM names", [], |r| r.get(0))?))
    }

    /// Insert or update a full glossary entry (matched by `japanese`). On update,
    /// only the value fields are refreshed (COALESCE keeps existing non-nulls);
    /// `source`/`status` are preserved so a confirmed entry isn't downgraded.
    /// Returns the row id.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_name_full(
        &self,
        japanese: &str,
        chinese: Option<&str>,
        english: Option<&str>,
        category: Option<&str>,
        notes: Option<&str>,
        source: &str,
        status: NameStatus,
    ) -> Result<i64> {
        let now = now_iso8601();
        self.db.write(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO names (japanese, chinese, english, category, notes, source, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT(japanese) DO UPDATE SET
                     chinese    = COALESCE(excluded.chinese, names.chinese),
                     english    = COALESCE(excluded.english, names.english),
                     category   = COALESCE(excluded.category, names.category),
                     notes      = COALESCE(excluded.notes, names.notes),
                     updated_at = excluded.updated_at",
                rusqlite::params![japanese, chinese, english, category, notes, source, status, now],
            )?;
            let id: i64 =
                tx.query_row("SELECT id FROM names WHERE japanese = ?1", [japanese], |r| r.get(0))?;
            tx.commit()?;
            Ok(id)
        })
    }

    /// Add an alias form for a name (idempotent on `japanese_form`).
    pub fn add_alias(&self, name_id: i64, japanese_form: &str) -> Result<()> {
        self.db.write(|c| {
            c.execute(
                "INSERT INTO name_aliases (name_id, japanese_form) VALUES (?1, ?2)
                 ON CONFLICT(japanese_form) DO NOTHING",
                rusqlite::params![name_id, japanese_form],
            )?;
            Ok(())
        })
    }

    /// Record a field change in `name_history`.
    pub fn record_history(&self, name_id: i64, field: &str, old: Option<&str>, new: Option<&str>) -> Result<()> {
        let now = now_iso8601();
        self.db.write(|c| {
            c.execute(
                "INSERT INTO name_history (name_id, field, old, new, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![name_id, field, old, new, now],
            )?;
            Ok(())
        })
    }

    /// List glossary entries (most-recently-updated first), capped at `limit`.
    pub fn list_names(&self, limit: i64) -> Result<Vec<GlossaryName>> {
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, japanese, chinese, english, category, notes, source, status
                 FROM names ORDER BY updated_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], |r| {
                Ok(GlossaryName {
                    id: r.get(0)?,
                    japanese: r.get(1)?,
                    chinese: r.get(2)?,
                    english: r.get(3)?,
                    category: r.get(4)?,
                    notes: r.get(5)?,
                    source: r.get(6)?,
                    status: r.get(7)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    /// All japanese forms (canonical + aliases) paired with their name id, for
    /// building a [`crate::names::Matcher`].
    pub fn glossary_forms(&self) -> Result<Vec<(String, i64)>> {
        self.db.read(|c| {
            let mut out = Vec::new();
            let mut a = c.prepare("SELECT japanese, id FROM names")?;
            for row in a.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                out.push(row?);
            }
            let mut b = c.prepare("SELECT japanese_form, name_id FROM name_aliases")?;
            for row in b.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                out.push(row?);
            }
            Ok(out)
        })
    }

    /// Look up a single entry by id.
    pub fn get_name(&self, id: i64) -> Result<Option<GlossaryName>> {
        self.db.read(|c| {
            c.query_row(
                "SELECT id, japanese, chinese, english, category, notes, source, status FROM names WHERE id = ?1",
                [id],
                |r| {
                    Ok(GlossaryName {
                        id: r.get(0)?,
                        japanese: r.get(1)?,
                        chinese: r.get(2)?,
                        english: r.get(3)?,
                        category: r.get(4)?,
                        notes: r.get(5)?,
                        source: r.get(6)?,
                        status: r.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
        })
    }
}
