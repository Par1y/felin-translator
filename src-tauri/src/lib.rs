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
            commands::scan_image_dir,
            commands::import_images_batch,
            commands::export_project,
            commands::import_project,
            commands::get_llm_config,
            commands::set_llm_config,
            commands::test_llm_connection,
            commands::csv_headers,
            commands::import_glossary_csv,
            commands::list_glossary,
            commands::set_global_name_tags,
            commands::set_global_name_enabled,
            commands::run_name_extraction,
            commands::list_extracted,
            commands::update_extracted,
            commands::reject_extracted,
            commands::confirm_extracted,
            commands::start_translation,
            commands::stop_translation,
            commands::translation_status,
            commands::retry_translation,
            commands::approve_tu,
            commands::set_tu_instruction,
            commands::retranslate_tu,
            commands::retranslate_tus,
            commands::get_translation_settings,
            commands::set_translation_settings,
            commands::get_guidelines,
            commands::set_guidelines,
            commands::list_tus_with_translations,
            commands::set_tu_source,
            commands::set_translation_text,
            commands::get_ocr_settings,
            commands::set_ocr_settings,
            commands::get_ocr_config,
            commands::set_ocr_config,
            commands::export_translations,
            commands::list_glossary_entries,
            commands::add_glossary_entry,
            commands::update_glossary_entry,
            commands::set_entry_enabled,
            commands::set_entry_tags,
            commands::delete_glossary_entry,
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

    // Sidecar path is user-managed: `[sidecar] bin` in felin.toml, or
    // `FELIN_SIDECAR` for dev. No guessed fallback — if it's unset or missing,
    // OCR import reports a clear error. `mock-ocr-cli` is never used here
    // (automated tests only).
    let sidecar = paths::resolve_sidecar(config.sidecar.bin.as_deref());
    match &sidecar {
        Some(p) if !p.exists() => {
            tracing::warn!(
                path = %p.display(),
                "OCR sidecar not found (set [sidecar] bin in felin.toml or FELIN_SIDECAR); OCR import will fail"
            );
        }
        None => {
            tracing::warn!(
                "OCR sidecar not configured (set [sidecar] bin in felin.toml or FELIN_SIDECAR); OCR import will fail"
            );
        }
        _ => {}
    }
    let sidecar_config = paths::resolve_sidecar_config(config.sidecar.config.as_deref());
    if let Some(p) = &sidecar_config {
        if !p.exists() {
            tracing::warn!(
                path = %p.display(),
                "OCR sidecar config not found; ocr-cli will use its own default (likely fatal)"
            );
        }
    }
    // A configured sidecar with no config at all is the classic exit-20 trap:
    // ocr-cli looks for its own `config.yaml` in the working directory and dies
    // if absent. Surface it at startup rather than at import time.
    if sidecar.is_some() && sidecar_config.is_none() {
        tracing::warn!(
            "OCR sidecar configured but no sidecar config ([sidecar] config / FELIN_SIDECAR_CONFIG); \
             ocr-cli will look for its own config.yaml and likely fail with exit 20"
        );
    }

    Ok(AppState {
        data_dir,
        config,
        global,
        sidecar,
        sidecar_config,
        project: Mutex::new(None),
        tasks: Mutex::new(HashMap::new()),
        translation: Mutex::new(None),
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
            // Write the self-documenting template (commented guidance, incl. the
            // user-managed `[sidecar] bin`/`config` keys) so advanced users can
            // discover and edit it; only ever written when the file is missing.
            if let Err(e) = std::fs::write(&path, TechConfig::default_template()) {
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
