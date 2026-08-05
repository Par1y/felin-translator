//! Tiny, transactional migration runner shared by both databases.
//!
//! Kept in-house (rather than pulling `refinery`) so the plan's specific
//! requirements are fully under our control: back up the project DB before an
//! upgrade, refuse to open a forward-versioned DB, and run each step in its own
//! transaction. Called during [`super::db::Db::open`] while the write
//! connection is the *only* open connection, so the pre-upgrade checkpoint+copy
//! is race-free.

use crate::error::{Error, Result};
use crate::util::now_iso8601;
use rusqlite::Connection;
use std::path::Path;

/// One migration step: a version number and the SQL that upgrades the schema
/// from `version - 1` to `version`.
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    pub version: i64,
    pub sql: &'static str,
}

/// Run all pending migrations against `conn`.
///
/// * `migrations` — ascending, contiguous from 1.
/// * `backup` — when true (project DB), copy an existing DB to
///   `<file>.bak-<current>` before applying any upgrade.
pub fn run(
    conn: &mut Connection,
    db_path: &Path,
    migrations: &[Migration],
    backup: bool,
) -> Result<()> {
    debug_assert!(
        migrations.windows(2).all(|w| w[0].version < w[1].version),
        "migrations must be strictly ascending"
    );

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _schema_version (
             version    INTEGER PRIMARY KEY,
             applied_at TEXT NOT NULL
         );",
    )?;

    let current: i64 =
        conn.query_row("SELECT COALESCE(MAX(version), 0) FROM _schema_version", [], |r| r.get(0))?;

    let supported_max = migrations.last().map(|m| m.version).unwrap_or(0);
    if current > supported_max {
        return Err(Error::SchemaTooNew { found: current, supported_max });
    }

    let pending: Vec<&Migration> = migrations.iter().filter(|m| m.version > current).collect();
    if pending.is_empty() {
        return Ok(());
    }

    if backup && current > 0 {
        backup_before_upgrade(conn, db_path, current)?;
    }

    for m in &pending {
        // IMMEDIATE takes the write lock at BEGIN, so concurrent processes (e.g.
        // two app instances on first launch of the shared glossary) serialize
        // here; re-checking the version under that lock prevents applying a
        // migration a peer already committed.
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| Error::migration(m.version, e.to_string()))?;
        let applied: i64 =
            tx.query_row("SELECT COALESCE(MAX(version), 0) FROM _schema_version", [], |r| r.get(0))
                .map_err(|e| Error::migration(m.version, e.to_string()))?;
        if applied >= m.version {
            let _ = tx.rollback();
            continue;
        }
        tx.execute_batch(m.sql).map_err(|e| Error::migration(m.version, e.to_string()))?;
        tx.execute(
            "INSERT INTO _schema_version (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![m.version, now_iso8601()],
        )
        .map_err(|e| Error::migration(m.version, e.to_string()))?;
        tx.commit().map_err(|e| Error::migration(m.version, e.to_string()))?;
        tracing::info!(version = m.version, "applied migration");
    }
    Ok(())
}

/// The highest version present in `migrations` (the version a freshly-created DB
/// ends at). Exposed for tests / diagnostics.
pub fn latest_version(migrations: &[Migration]) -> i64 {
    migrations.last().map(|m| m.version).unwrap_or(0)
}

fn backup_before_upgrade(conn: &Connection, db_path: &Path, current: i64) -> Result<()> {
    // Fold the WAL back into the main file so a single-file copy is complete.
    // We are the only open connection here, so a TRUNCATE checkpoint fully
    // drains the WAL.
    // Fold the WAL into the main file so a single-file copy is complete. The
    // checkpoint's first result column is a "busy" flag: 0 means it fully
    // drained. If it didn't (or errored), copy the -wal/-shm sidecars too so the
    // backup is still restorable.
    let fully_checkpointed = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0))
        .map(|busy| busy == 0)
        .unwrap_or(false);
    if !fully_checkpointed {
        tracing::warn!("WAL not fully checkpointed before backup; copying -wal/-shm too");
        copy_if_exists(&wal_path(db_path))?;
        copy_if_exists(&shm_path(db_path))?;
    }
    let file_name = db_path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| Error::migration(current, "database path has no file name"))?;
    let bak = db_path.with_file_name(format!("{file_name}.bak-{current}"));
    std::fs::copy(db_path, &bak)?;
    tracing::info!(backup = %bak.display(), "backed up project DB before migration");
    Ok(())
}

fn copy_if_exists(p: &Path) -> Result<()> {
    if p.exists() {
        let bak = p.with_extension(format!(
            "{}.presplit-bak",
            p.extension().and_then(|s| s.to_str()).unwrap_or("")
        ));
        std::fs::copy(p, bak)?;
    }
    Ok(())
}

fn wal_path(db: &Path) -> std::path::PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push("-wal");
    s.into()
}

fn shm_path(db: &Path) -> std::path::PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push("-shm");
    s.into()
}
