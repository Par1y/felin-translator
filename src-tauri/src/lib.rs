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
    // `init_tracing` must reflect the `[debug]` switch, but the full config is
    // only loaded into AppState later (inside `.setup` → `build_state`). Resolve
    // the data root now and read the flag early so the default log level honors
    // felin.toml `[debug].enabled` (build_state re-loads the same file later).
    let data_dir = paths::data_root();
    let _ = std::fs::create_dir_all(&data_dir);
    let debug_enabled = load_tech_config(&data_dir).debug.enabled;
    init_tracing(debug_enabled);

    tauri::Builder::default()
        .setup(|app| {
            app.manage(build_state()?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::create_project,
            commands::rename_project,
            commands::delete_project,
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
            commands::csv_preview,
            commands::import_glossary_csv,
            commands::list_glossary,
            commands::set_global_name_tags,
            commands::set_global_name_enabled,
            commands::delete_global_names,
            commands::run_name_extraction,
            commands::list_extracted,
            commands::update_extracted,
            commands::update_extracted_tags,
            commands::auto_tag_extracted,
            commands::apply_extracted_tags,
            commands::reject_extracted,
            commands::reject_extracted_batch,
            commands::confirm_extracted,
            commands::confirm_extracted_batch,
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
            commands::get_prompt_config,
            commands::set_prompt_config,
            commands::list_tus_with_translations,
            commands::set_tu_source,
            commands::set_translation_text,
            commands::delete_tus,
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
            commands::delete_glossary_entries,
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

    // Seed the runtime-effective prompt templates from the loaded config;
    // `set_prompt_config` swaps this in place (no restart needed).
    let prompt = config.prompt.clone();

    Ok(AppState {
        data_dir,
        config,
        prompt: Mutex::new(prompt),
        global,
        sidecar,
        sidecar_config,
        project: Mutex::new(None),
        tasks: Mutex::new(HashMap::new()),
        translation: Mutex::new(None),
    })
}

/// Load technical config from `<data_dir>/felin.toml`. Writes the
/// self-documenting default file (incl. the factory `[prompt]` templates) when
/// none exists, and heals a legacy file that predates `[prompt]` by appending
/// the section — so the prompt text the LLM uses always comes from the config
/// file, never silently empty or baked into the runtime.
fn load_tech_config(dir: &std::path::Path) -> felin_core::config::TechConfig {
    use felin_core::config::TechConfig;
    let path = dir.join("felin.toml");
    TechConfig::load_from_disk(&path).0
}

fn init_tracing(debug_enabled: bool) {
    use tracing_subscriber::{fmt, EnvFilter};
    // Default level honors felin.toml `[debug]`: off → `info` only (felin_core
    // debug/trace stay silent); on → also surface felin_core's debug traces.
    // `RUST_LOG` always wins when set.
    let default_filter = if debug_enabled {
        "info,felin_core=debug,felin_translator_lib=debug"
    } else {
        "info"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let _ = fmt().with_env_filter(filter).try_init();
}
