//! Portable path resolution.
//!
//! The app keeps ALL internal data (the global glossary plus every project's DB
//! and OCR products) next to the software — "green", never in the OS appdata
//! dir. User source files are read in place and are never copied here.

use std::path::{Path, PathBuf};

/// Dev override env vars (mirroring `FELIN_DATA_DIR`): point the OCR backend
/// (sidecar binary) and its config anywhere for a dev run, e.g.
/// `../ocr-router/bin/ocr-cli`, without editing `felin.toml`.
const ENV_SIDECAR: &str = "FELIN_SIDECAR";
const ENV_SIDECAR_CONFIG: &str = "FELIN_SIDECAR_CONFIG";

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
        if let Some(parent) = std::path::Path::new(&appimage).parent() {
            return parent.join("felin-data");
        }
    }
    exe_dir().unwrap_or_else(|| PathBuf::from(".")).join("felin-data")
}

/// Resolve the OCR backend (sidecar) binary path from *user-managed* sources
/// only: `[sidecar] bin` in `felin.toml`, else `FELIN_SIDECAR`. Returns `None`
/// when neither is set — the caller reports a clear "not configured" error
/// rather than guessing a location. The expected binary is the real `ocr-cli`
/// from the ocr-router project (`mock-ocr-cli` is automated-test-only and never
/// the production backend).
pub fn resolve_sidecar(cfg_bin: Option<&str>) -> Option<PathBuf> {
    cfg_bin
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os(ENV_SIDECAR).map(PathBuf::from))
}

/// Resolve the OCR backend config file path from user-managed sources only:
/// `[sidecar] config` in `felin.toml`, else `FELIN_SIDECAR_CONFIG`. `None`
/// means "don't pass `-c`" — `ocr-cli` then uses its own default `config.yaml`
/// (the ocr-router project's own user-managed config, which holds provider
/// keys).
pub fn resolve_sidecar_config(cfg_config: Option<&str>) -> Option<PathBuf> {
    cfg_config
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os(ENV_SIDECAR_CONFIG).map(PathBuf::from))
}
