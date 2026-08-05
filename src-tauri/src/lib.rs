//! Felin Translator — Tauri v2 application shell.
//!
//! Intentionally thin: it owns process/window lifecycle, resolves paths, opens
//! the global glossary DB, and exposes [`commands`] over `invoke`. All domain
//! logic lives in the `felin-core` crate.

mod commands;
mod paths;
mod state;

use felin_core::storage::GlobalDb;
use state::AppState;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    tauri::Builder::default()
        .setup(|app| {
            app.manage(build_state()?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::create_project,
            commands::open_project,
            commands::close_project,
            commands::current_project,
            commands::list_projects,
            commands::list_chapters,
            commands::list_paragraphs,
            commands::list_tus,
            commands::segment_project,
            commands::import_txt_file,
            commands::import_ocr,
            commands::cancel_import,
            commands::export_project,
            commands::import_project,
            commands::get_llm_config,
            commands::set_llm_config,
            commands::csv_headers,
            commands::import_glossary_csv,
            commands::list_glossary,
            commands::run_name_extraction,
            commands::list_extracted,
            commands::update_extracted,
            commands::reject_extracted,
            commands::confirm_extracted,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Felin Translator");
}

/// Build managed state: resolve the portable data root (next to the executable,
/// or `FELIN_DATA_DIR`), open the global glossary DB, and locate the OCR sidecar.
/// Nothing is written to the OS appdata dir.
fn build_state() -> Result<AppState, Box<dyn std::error::Error>> {
    let data_dir = paths::data_root();
    std::fs::create_dir_all(&data_dir)?;

    let config = load_tech_config(&data_dir);
    let global = GlobalDb::open_with(
        &data_dir.join("glossary.db"),
        felin_core::storage::DbTuning {
            read_pool_size: config.db.read_pool_size,
            busy_timeout_ms: config.db.busy_timeout_ms,
        },
    )?;

    let sidecar =
        paths::resolve_sidecar().unwrap_or_else(|_| std::path::PathBuf::from(paths::SIDECAR_BIN));
    if !sidecar.exists() {
        tracing::warn!(
            path = %sidecar.display(),
            "OCR sidecar not found next to the executable; OCR import will fail until it is bundled"
        );
    }

    Ok(AppState {
        data_dir,
        config,
        global,
        sidecar,
        project: Mutex::new(None),
        tasks: Mutex::new(HashMap::new()),
    })
}

/// Load technical config from `<data_dir>/felin.toml`, writing a documented
/// default file if none exists so advanced users can discover and edit it.
fn load_tech_config(dir: &std::path::Path) -> felin_core::config::TechConfig {
    use felin_core::config::TechConfig;
    let path = dir.join("felin.toml");
    match std::fs::read_to_string(&path) {
        Ok(s) => TechConfig::from_toml_str(&s).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "invalid felin.toml; using defaults");
            TechConfig::default()
        }),
        Err(_) => {
            let c = TechConfig::default();
            if let Err(e) = std::fs::write(&path, c.to_toml_string()) {
                tracing::warn!(error = %e, "could not write default felin.toml");
            }
            c
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,felin_core=debug,felin_translator_lib=debug"));
    let _ = fmt().with_env_filter(filter).try_init();
}
