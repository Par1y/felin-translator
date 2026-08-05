//! Single-open project lock (`project.lock`).
//!
//! Prevents two app windows/processes from opening the same project (the plan's
//! "校对：单用户单线程 … project.lock 防双开"). Uses an advisory file lock via
//! the standard library (`File::try_lock`, stable since Rust 1.89), which the OS
//! releases automatically if the process exits or crashes — avoiding the
//! stale-lock problem of a plain PID file.

use crate::error::{Error, Result};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// An RAII guard holding the exclusive lock on a project. Dropping it releases
/// the lock (closing the file handle does so even on panic/crash).
#[derive(Debug)]
pub struct ProjectLock {
    // Held to keep the advisory lock alive; released on drop.
    _file: File,
    path: PathBuf,
}

impl ProjectLock {
    /// Try to acquire the lock for the project rooted at `project_root`
    /// (creates the directory and `project.lock` if needed).
    ///
    /// Returns [`Error::ProjectLocked`] if another process already holds it.
    pub fn acquire(project_root: &Path) -> Result<Self> {
        std::fs::create_dir_all(project_root)?;
        let path = project_root.join("project.lock");
        let file =
            OpenOptions::new().create(true).read(true).write(true).truncate(false).open(&path)?;

        match file.try_lock() {
            Ok(()) => {
                // Record the owning PID for diagnostics (advisory only).
                let _ = file.set_len(0);
                let _ = writeln!(&file, "{}", std::process::id());
                Ok(Self { _file: file, path })
            }
            Err(e) => {
                // `TryLockError` converts to an io::Error; `WouldBlock` means the
                // lock is held elsewhere. Match on the kind to avoid depending on
                // the exact enum variant names.
                let io_err: std::io::Error = e.into();
                if io_err.kind() == std::io::ErrorKind::WouldBlock {
                    Err(Error::ProjectLocked { path })
                } else {
                    Err(Error::Io(io_err))
                }
            }
        }
    }

    /// Path to the lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}
