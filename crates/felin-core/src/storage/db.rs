//! Low-level database handle: one serialized write connection plus a pool of
//! read-only connections, matching the plan's "single writer (Mutex) + pooled
//! readers, all in WAL" design.

use crate::error::{Error, Result};
use crate::storage::migrations::{self, Migration};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// SQLite connection tuning (from [`crate::config`]).
#[derive(Debug, Clone, Copy)]
pub struct DbTuning {
    pub read_pool_size: u32,
    pub busy_timeout_ms: u32,
}

impl Default for DbTuning {
    fn default() -> Self {
        Self { read_pool_size: 8, busy_timeout_ms: 5_000 }
    }
}

/// A handle to one SQLite database file.
///
/// All writes are serialized through a single connection behind a `Mutex`;
/// reads use a pool of connections pinned `query_only` so a stray write can
/// never bypass the write lock. `Db` is `Send + Sync` and is meant to be shared
/// via `Arc` (e.g. as Tauri managed state).
pub struct Db {
    path: PathBuf,
    write: Arc<Mutex<Connection>>,
    read_pool: Pool<SqliteConnectionManager>,
}

impl Db {
    /// Open (creating if needed) the database at `path`, apply pragmas, and run
    /// `migrations`. `backup_on_upgrade` copies an existing DB before upgrading
    /// (true for project DBs). `tuning` sets the read-pool size and busy timeout.
    pub fn open(
        path: &Path,
        migrations: &[Migration],
        backup_on_upgrade: bool,
        tuning: DbTuning,
    ) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // (1) Open the single write connection and configure it.
        let mut write = Connection::open(path)?;
        configure_write_conn(&write, tuning.busy_timeout_ms)?;

        // (2) Migrate while this is the only open connection, so the pre-upgrade
        //     checkpoint + file copy cannot race another connection.
        migrations::run(&mut write, path, migrations, backup_on_upgrade)?;

        // (3) Stand up the read pool. Pool connections are read-write at the OS
        //     level (avoids the read-only + WAL open pitfall) but pinned
        //     `query_only=ON`, so they are read-only in practice.
        let busy = tuning.busy_timeout_ms;
        let manager =
            SqliteConnectionManager::file(path).with_init(move |c| configure_read_conn(c, busy));
        let read_pool = Pool::builder().max_size(tuning.read_pool_size.max(1)).build(manager)?;

        Ok(Self { path: path.to_path_buf(), write: Arc::new(Mutex::new(write)), read_pool })
    }

    /// Filesystem path of this database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run `f` with exclusive write access. The closure receives `&mut Connection`
    /// so it can open transactions.
    pub fn write<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        // Recover from a poisoned lock: a previous panic while holding the write
        // connection should not permanently brick the database handle.
        let mut guard = self.write.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut guard)
    }

    /// Run `f` with a pooled, read-only connection.
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.read_pool.get()?;
        f(&conn)
    }
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").field("path", &self.path).finish_non_exhaustive()
    }
}

fn configure_write_conn(conn: &Connection, busy_timeout_ms: u32) -> Result<()> {
    conn.busy_timeout(std::time::Duration::from_millis(busy_timeout_ms as u64))?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    // journal_mode returns the resulting mode; verify WAL actually took effect.
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(Error::migration(0, format!("could not enable WAL (got journal_mode={mode})")));
    }
    Ok(())
}

fn configure_read_conn(conn: &mut Connection, busy_timeout_ms: u32) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_millis(busy_timeout_ms as u64))?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA query_only = ON;")
}
