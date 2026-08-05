//! Portable path resolution.
//!
//! The app keeps ALL internal data (the global glossary plus every project's DB
//! and OCR products) next to the software — "green", never in the OS appdata
//! dir. User source files are read in place and are never copied here.

use std::path::{Path, PathBuf};

/// The sidecar binary name (without the target-triple suffix Tauri strips at
/// bundle time). Swap to the real `ocr-cli` once ocr-router implements the
/// contract; the app looks for exactly this name beside its executable.
pub const SIDECAR_BIN: &str = "mock-ocr-cli";

/// Directory containing the executable, accounting for the `deps/` dir used by
/// `cargo run`/tests.
fn exe_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    if dir.ends_with("deps") {
        dir.parent().map(Path::to_path_buf)
    } else {
        Some(dir)
    }
}

/// Resolve the portable data root (holds `glossary.db` + `projects/`):
/// 1. `FELIN_DATA_DIR` if set (dev/testing override);
/// 2. next to the `.AppImage` file when running as one (its mount is read-only);
/// 3. otherwise `<exe dir>/felin-data`.
pub fn data_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("FELIN_DATA_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        if let Some(parent) = Path::new(&appimage).parent() {
            return parent.join("felin-data");
        }
    }
    exe_dir().unwrap_or_else(|| PathBuf::from(".")).join("felin-data")
}

/// Resolve the bundled sidecar's on-disk path. Tauri places `externalBin`
/// entries next to the main executable (triple suffix stripped), so this mirrors
/// Tauri's own resolver — no shell plugin required.
pub fn resolve_sidecar() -> std::io::Result<PathBuf> {
    let base = exe_dir().ok_or_else(|| std::io::Error::other("cannot resolve executable directory"))?;
    let file = if cfg!(windows) { format!("{SIDECAR_BIN}.exe") } else { SIDECAR_BIN.to_string() };
    Ok(base.join(file))
}
