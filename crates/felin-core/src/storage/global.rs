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
pub const GLOBAL_MIGRATIONS: &[Migration] = &[
    Migration { version: 1, sql: include_str!("migrations/global/0001_init.sql") },
    Migration { version: 2, sql: include_str!("migrations/global/0002_tags_enabled.sql") },
    Migration { version: 3, sql: include_str!("migrations/global/0003_remove_aliases.sql") },
];

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
                "SELECT id, japanese, chinese, english, category, notes, source, status, tags, enabled
                 FROM names ORDER BY updated_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], row_to_name)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    /// Search the global pool by `japanese` / `chinese` / `english` / a tag,
    /// most-recently-updated first, capped at `limit`. `q` is matched as a
    /// substring (tags live in a JSON array column, so a LIKE against it covers
    /// tag filtering too). Empty `q` lists everything (`%%` matches all rows).
    pub fn search_names(&self, q: &str, limit: i64) -> Result<Vec<GlossaryName>> {
        let pat = format!("%{q}%");
        self.db.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, japanese, chinese, english, category, notes, source, status, tags, enabled
                 FROM names
                 WHERE japanese LIKE ?1 OR chinese LIKE ?1 OR english LIKE ?1 OR tags LIKE ?1
                 ORDER BY updated_at DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![pat, limit], row_to_name)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    /// Set a global entry's tag array (JSON).
    pub fn set_name_tags(&self, id: i64, tags: &[String]) -> Result<()> {
        self.db.write(|c| {
            c.execute(
                "UPDATE names SET tags = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![serde_json::to_string(tags).unwrap_or_else(|_| "[]".into()), now_iso8601(), id],
            )?;
            Ok(())
        })
    }

    /// Toggle a global entry's enabled flag.
    pub fn set_name_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        self.db.write(|c| {
            c.execute(
                "UPDATE names SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![enabled as i64, now_iso8601(), id],
            )?;
            Ok(())
        })
    }

    /// All canonical japanese forms paired with their name id, for building a
    /// [`crate::names::Matcher`].
    pub fn glossary_forms(&self) -> Result<Vec<(String, i64)>> {
        self.db.read(|c| {
            let mut stmt = c.prepare("SELECT japanese, id FROM names")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        })
    }

    /// Look up a single entry by id.
    pub fn get_name(&self, id: i64) -> Result<Option<GlossaryName>> {
        self.db.read(|c| {
            c.query_row(
                "SELECT id, japanese, chinese, english, category, notes, source, status, tags, enabled
                 FROM names WHERE id = ?1",
                [id],
                row_to_name,
            )
            .optional()
            .map_err(Into::into)
        })
    }

    /// Batch-delete global entries by id (`name_history` rows cascade via FK;
    /// `name_aliases` was dropped in migration v3). Returns the number actually
    /// deleted. Project small-glossary `name_global_id` pointers are *not* touch
    /// here — callers clear those separately (see
    /// [`ProjectDb::clear_global_provenance`]).
    pub fn delete_names(&self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        self.db.write(|c| {
            let ph = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
            let sql = format!("DELETE FROM names WHERE id IN ({ph})");
            Ok(c.execute(&sql, rusqlite::params_from_iter(ids.iter()))?)
        })
    }
}

/// Map a global `names` row (column order: id, japanese, chinese, english,
/// category, notes, source, status, tags, enabled) to a [`GlossaryName`].
fn row_to_name(r: &rusqlite::Row<'_>) -> rusqlite::Result<GlossaryName> {
    let tags_json: String = r.get(8)?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    Ok(GlossaryName {
        id: r.get(0)?,
        japanese: r.get(1)?,
        chinese: r.get(2)?,
        english: r.get(3)?,
        category: r.get(4)?,
        notes: r.get(5)?,
        source: r.get(6)?,
        status: r.get(7)?,
        tags,
        enabled: r.get::<_, i64>(9)? != 0,
    })
}
