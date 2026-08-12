//! Tauri command surface — the boundary the React frontend calls over `invoke`.
//!
//! Commands return `Result<T, String>` (the `Err` string reaches the frontend).
//! Long-running OCR import runs on a dedicated thread + runtime and reports via
//! events (`ocr://progress`, `ocr://done`, `ocr://error`); see [`import_ocr`].
//!
//! Data model: all internal data (glossary + every project's DB and OCR
//! products) lives under the portable `data_dir` next to the software. User
//! source files are read in place. Projects can be moved/backed up as a single
//! verified archive via [`export_project`] / [`import_project`].

use crate::state::{AppState, OpenProject};
use felin_core::archive;
use felin_core::config::PromptConfig;
use felin_core::llm::{LlmClient, prompt::TranslateRequest};
use felin_core::names;
use felin_core::ocr::contract::PageStatus;
use felin_core::ocr::sidecar::run_extract;
use felin_core::ocr::{
    batch::{ingest_batch_txts, run_batch, BatchArgs, BatchEvent},
    config::{apply_and_write, read_config_file, OcrConfig},
    select::{select_images, ImageMatchRule},
    ingest_from_manifest, read_manifest, ExtractArgs, ExtractOutcome, ProgressEvent,
};
use felin_core::pipeline::{run_pipeline, LlmTranslator, PipelineEvent, RunConfig};
use felin_core::storage::{DbTuning, ProjectDb, ProjectLock};
use felin_core::types::{
    Chapter, ExtractedName, ExtractedNameStatus, FileSelection, GlossaryEntry, GlossaryName,
    NameStatus, OcrSettings, Paragraph, TranslationExport, TranslationSettings, Tu,
    TuWithTranslation,
};
use felin_core::util::now_iso8601;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{mpsc, watch, Notify};

// PLACEHOLDER_TYPES

#[derive(Serialize, Clone)]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub sidecar: String,
    pub sidecar_present: bool,
    pub ocr_config_path: String,
    pub ocr_config_present: bool,
    pub glossary_names: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ProjectSummary {
    pub slug: String,
    pub name: String,
    pub created_at: String,
}

#[derive(Serialize, Clone)]
pub struct ProgressPayload {
    pub task_id: String,
    pub event: ProgressEvent,
}

#[derive(Serialize, Clone)]
pub struct ImportResult {
    pub task_id: String,
    pub outcome: String,
    pub pages_ok: usize,
    pub pages_failed: usize,
    pub failed_pages: Vec<i64>,
    pub paragraphs: usize,
    pub chapter_id: i64,
}

#[derive(Serialize, Clone)]
pub struct ErrorPayload {
    pub task_id: String,
    pub message: String,
}

#[derive(Serialize, Clone)]
pub struct TxtImportResult {
    pub chapter_id: i64,
    pub paragraphs: usize,
}

#[derive(Serialize, Clone)]
pub struct ExportResult {
    pub task_id: String,
    pub archive: String,
    pub sha256: String,
    pub bytes: u64,
    pub files: usize,
}

#[derive(Serialize, Clone)]
pub struct ExportProgressPayload {
    pub task_id: String,
    pub event: felin_core::archive::ArchiveProgress,
}

#[derive(Serialize, Clone)]
pub struct SegmentResult {
    pub chapters: usize,
    pub tus: usize,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run `f` against the currently-open project, or return a friendly error.
fn with_project<T>(
    state: &AppState,
    f: impl FnOnce(&OpenProject) -> felin_core::Result<T>,
) -> Result<T, String> {
    let guard = state.project_guard();
    let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
    f(proj).map_err(|e| e.to_string())
}

/// Filesystem-safe slug: keep alphanumerics (incl. CJK), collapse other runs to
/// a single '-', trim, cap length.
fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in name.trim().chars() {
        if ch.is_alphanumeric() {
            slug.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-').chars().take(64).collect::<String>();
    if slug.is_empty() {
        format!("project-{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
    } else {
        slug
    }
}

/// True if `slug` is a single normal path component (guards `open_project`
/// against traversal like `../../etc`).
fn is_valid_slug(slug: &str) -> bool {
    let mut it = Path::new(slug).components();
    matches!(it.next(), Some(Component::Normal(_))) && it.next().is_none()
}

fn meta_path(root: &Path) -> PathBuf {
    root.join("project.json")
}

/// Read and parse a project's `project.json` summary.
fn read_project_json(root: &Path) -> Result<ProjectSummary, String> {
    serde_json::from_slice(&std::fs::read(meta_path(root)).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

/// Read a regular file, rejecting non-files (FIFO/device) and anything over `max`.
fn read_regular_capped(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(format!("not a regular file: {}", path.display()));
    }
    if meta.len() > max {
        return Err(format!("file is {} bytes, exceeding the {} byte limit", meta.len(), max));
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

fn outcome_str(o: ExtractOutcome) -> String {
    match o {
        ExtractOutcome::AllOk => "all_ok",
        ExtractOutcome::Partial => "partial",
        ExtractOutcome::Cancelled => "cancelled",
    }
    .to_string()
}

fn db_tuning(state: &AppState) -> DbTuning {
    DbTuning {
        read_pool_size: state.config.db.read_pool_size,
        busy_timeout_ms: state.config.db.busy_timeout_ms,
    }
}

// PLACEHOLDER_COMMANDS

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: state.data_dir.display().to_string(),
        sidecar: state.sidecar.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
        sidecar_present: state.sidecar.as_ref().is_some_and(|p| p.exists()),
        ocr_config_path: state
            .sidecar_config
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        ocr_config_present: state.sidecar_config.as_ref().is_some_and(|p| p.exists()),
        glossary_names: state.global.count_names().unwrap_or(0),
    }
}

#[tauri::command]
pub fn create_project(state: State<'_, AppState>, name: String) -> Result<ProjectSummary, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("project name must not be empty".into());
    }
    let slug = slugify(&name);
    let root = state.projects_dir().join(&slug);
    if meta_path(&root).exists() {
        return Err(format!("a project with slug '{slug}' already exists"));
    }

    let lock = ProjectLock::acquire(&root).map_err(|e| e.to_string())?;
    let db = ProjectDb::open_with(&root.join("project.db"), db_tuning(&state)).map_err(|e| e.to_string())?;

    let created_at = now_iso8601();
    let summary = ProjectSummary { slug: slug.clone(), name: name.clone(), created_at: created_at.clone() };
    std::fs::write(meta_path(&root), serde_json::to_vec_pretty(&summary).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    db.set_project_name(&name).map_err(|e| e.to_string())?;
    db.set_setting("created_at", &created_at).map_err(|e| e.to_string())?;

    *state.project_guard() = Some(OpenProject { slug, name, root, db: Arc::new(db), lock });
    Ok(summary)
}

#[tauri::command]
pub fn open_project(state: State<'_, AppState>, slug: String) -> Result<ProjectSummary, String> {
    if !is_valid_slug(&slug) {
        return Err(format!("invalid project id: {slug:?}"));
    }
    let root = state.projects_dir().join(&slug);
    if !meta_path(&root).exists() {
        return Err(format!("project '{slug}' not found"));
    }
    let summary: ProjectSummary =
        serde_json::from_slice(&std::fs::read(meta_path(&root)).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;

    let lock = ProjectLock::acquire(&root).map_err(|e| e.to_string())?;
    let db = ProjectDb::open_with(&root.join("project.db"), db_tuning(&state)).map_err(|e| e.to_string())?;

    *state.project_guard() =
        Some(OpenProject { slug, name: summary.name.clone(), root, db: Arc::new(db), lock });
    Ok(summary)
}

#[tauri::command]
pub fn close_project(state: State<'_, AppState>) {
    *state.project_guard() = None; // drops db (Arc) then lock
}

#[tauri::command]
pub fn current_project(state: State<'_, AppState>) -> Option<ProjectSummary> {
    let guard = state.project_guard();
    guard.as_ref().map(|p| ProjectSummary {
        slug: p.slug.clone(),
        name: p.name.clone(),
        created_at: String::new(),
    })
}

#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Result<Vec<ProjectSummary>, String> {
    let dir = state.projects_dir();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(out); // no projects yet
    };
    for entry in entries.flatten() {
        if let Ok(bytes) = std::fs::read(meta_path(&entry.path())) {
            if let Ok(summary) = serde_json::from_slice::<ProjectSummary>(&bytes) {
                out.push(summary);
            }
        }
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(out)
}

/// Rename a project's **display name** only — the disk directory and slug are
/// never touched. Updates `project.json` (preserving slug/created_at) and the
/// project's `project_name` setting; if the renamed project is the currently
/// open one, the in-memory `OpenProject.name` is synced so the main title
/// updates immediately. Returns the updated summary.
#[tauri::command]
pub fn rename_project(state: State<'_, AppState>, slug: String, name: String) -> Result<ProjectSummary, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("项目名称不能为空".into());
    }
    if !is_valid_slug(&slug) {
        return Err(format!("无效的项目 id：{slug:?}"));
    }
    let root = state.projects_dir().join(&slug);
    if !meta_path(&root).exists() {
        return Err(format!("项目 '{slug}' 不存在"));
    }

    let mut summary = read_project_json(&root)?;
    summary.name = name.clone();
    std::fs::write(meta_path(&root), serde_json::to_vec_pretty(&summary).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;

    // Keep the `project_name` setting consistent. When this project is open,
    // reuse its live DB and sync the open project's name; otherwise open the DB
    // briefly just to update the setting.
    let mut guard = state.project_guard();
    match guard.as_mut() {
        Some(p) if p.slug == slug => {
            p.db.set_project_name(&name).map_err(|e| e.to_string())?;
            p.name = name;
        }
        _ => {
            let db = ProjectDb::open_with(&root.join("project.db"), db_tuning(&state))
                .map_err(|e| e.to_string())?;
            db.set_project_name(&name).map_err(|e| e.to_string())?;
        }
    }
    drop(guard);

    Ok(summary)
}

/// Delete an entire project directory (`<projects_dir>/<slug>`, including its
/// OCR products). Archives are never touched. If the deleted project is the
/// currently open one, it is closed first (releasing the DB + single-open lock
/// and clearing the open-project slot).
#[tauri::command]
pub fn delete_project(state: State<'_, AppState>, slug: String) -> Result<(), String> {
    if !is_valid_slug(&slug) {
        return Err(format!("无效的项目 id：{slug:?}"));
    }
    let root = state.projects_dir().join(&slug);
    if !root.is_dir() {
        return Err(format!("项目 '{slug}' 不存在"));
    }

    {
        let mut guard = state.project_guard();
        if guard.as_ref().is_some_and(|p| p.slug == slug) {
            *guard = None; // drops the open db (Arc) then the lock
        }
    }

    std::fs::remove_dir_all(&root).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_chapters(state: State<'_, AppState>) -> Result<Vec<Chapter>, String> {
    with_project(&state, |p| p.db.list_chapters())
}

#[tauri::command]
pub fn list_paragraphs(state: State<'_, AppState>, chapter_id: i64) -> Result<Vec<Paragraph>, String> {
    with_project(&state, |p| p.db.list_paragraphs(chapter_id))
}

#[tauri::command]
pub fn list_tus(state: State<'_, AppState>, chapter_id: i64) -> Result<Vec<Tu>, String> {
    with_project(&state, |p| p.db.list_tus(chapter_id))
}

// ----- translation pipeline (plan step 8) ---------------------------------

/// A progress event relayed from the pipeline to the frontend.
#[derive(Serialize, Clone)]
pub struct TranslationProgressPayload {
    pub task_id: String,
    pub event: PipelineEvent,
}

#[derive(Serialize, Clone)]
pub struct TranslationDonePayload {
    pub task_id: String,
}

/// Per-status TU count for the status bar.
#[derive(Serialize, Clone)]
pub struct StatusCount {
    pub status: String,
    pub count: i64,
}

#[derive(Serialize, Clone)]
pub struct TranslationStatusView {
    pub running: bool,
    pub task_id: Option<String>,
    pub workers: i64,
    pub window: i64,
    pub active_chapters: Vec<i64>,
    pub counts: Vec<StatusCount>,
}

/// Start a translation pass over the open project's eligible TUs. Returns a
/// `task_id` immediately; progress arrives via `translation://progress` and
/// completion via `translation://done` / `translation://error`. Stop with
/// [`stop_translation`]; retry with [`retry_translation`] / [`retranslate_tu`]
/// (which wake a running scheduler via the stored [`Notify`]).
#[tauri::command]
pub fn start_translation(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    match spawn_translation(&app, &state)? {
        Some(task_id) => Ok(task_id),
        None => Err("translation already running".into()),
    }
}

/// Spawn a translation run over the open project's eligible TUs. Returns
/// `Some(task_id)` when a run was started, `None` when one is already active.
/// Progress arrives via `translation://progress`, completion via
/// `translation://done` / `translation://error`. Shared by `start_translation`
/// (which errors on "already running") and the retranslate/retry commands
/// (which start a run on demand so requeued TUs are picked up even when the
/// pipeline is idle).
fn spawn_translation(app: &tauri::AppHandle, state: &AppState) -> Result<Option<String>, String> {
    // One run at a time; the run thread deregisters itself when it finishes.
    if state.translation_guard().is_some() {
        return Ok(None);
    }
    // Capture everything the run thread needs under the project lock. Glossary
    // data for prompt injection is fetched by the pipeline itself from the
    // project's enabled small-glossary entries.
    let (db, settings, llm_cfg) = {
        let guard = state.project_guard();
        let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
        let settings = proj.db.get_translation_settings().map_err(|e| e.to_string())?;
        let llm_cfg = load_llm_config(&proj.db, &state.config.llm)?;
        (Arc::clone(&proj.db), settings, llm_cfg)
    };
    // The runtime-effective prompt templates (updated in place by the settings
    // page's `set_prompt_config`, so this run uses the latest text).
    let prompt = state.prompt_config();
    let cfg = RunConfig {
        workers: settings.workers as usize,
        window: settings.window as usize,
        memory_dedup: settings.memory_dedup,
        stop_aborts_inflight: settings.stop_aborts_inflight,
        queue_capacity: state.config.pipeline.queue_capacity,
        context_max_chars: state.config.pipeline.context_max_chars,
        guidelines_max_chars: state.config.pipeline.guidelines_max_chars,
        system_template: prompt.translation_system.clone(),
        user_template: prompt.translation_user.clone(),
    };
    drop(prompt);
    // Build the translator now so config errors reject the invoke synchronously
    // (before any task_id / events exist). The client shares the app-wide rate
    // limiter, so this run's workers queue with every other LLM feature.
    let translator = Arc::new(LlmTranslator {
        client: LlmClient::with_limiter(llm_cfg, Arc::clone(&state.llm_limiter)).map_err(|e| e.to_string())?,
    });

    let task_id = uuid::Uuid::new_v4().to_string();
    let (stop_tx, stop_rx) = watch::channel(false);
    let wake = Arc::new(Notify::new());
    *state.translation_guard() = Some(crate::state::TranslationRun {
        task_id: task_id.clone(),
        stop: stop_tx,
        wake: Arc::clone(&wake),
    });

    let app_thread = app.clone();
    let tid = task_id.clone();
    std::thread::spawn(move || {
        // RAII: always clear the translation-run slot, on every exit path, but
        // only if it's still *this* run (a later run must not be clobbered).
        struct Dereg {
            app: tauri::AppHandle,
            tid: String,
        }
        impl Drop for Dereg {
            fn drop(&mut self) {
                if let Some(st) = self.app.try_state::<AppState>() {
                    let mut g = st.translation_guard();
                    if g.as_ref().is_some_and(|r| r.task_id == self.tid) {
                        *g = None;
                    }
                }
            }
        }
        let _dereg = Dereg { app: app_thread.clone(), tid: tid.clone() };

        // The pipeline workers call blocking rusqlite (DB claim/save) inside
        // async tasks, so a multi-threaded runtime is required: a blocking call
        // only stalls one of its OS threads, whereas current_thread would freeze
        // every worker and the sidecar streaming. 2 worker threads is plenty —
        // the N pipeline workers are tokio tasks interleaving on them.
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = app_thread.emit(
                    "translation://error",
                    ErrorPayload { task_id: tid, message: format!("could not start runtime: {e}") },
                );
                return;
            }
        };
        // catch_unwind so a panic still produces a terminal event.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rt.block_on(async {
                let (ev_tx, ev_rx) = mpsc::unbounded_channel::<PipelineEvent>();
                tokio::join!(
                    run_pipeline(db, translator, cfg, stop_rx, wake, ev_tx),
                    forward_translation_events(&app_thread, &tid, ev_rx),
                )
            })
        }));
        match result {
            Ok((Ok(()), _)) => {
                let _ = app_thread.emit("translation://done", TranslationDonePayload { task_id: tid });
            }
            Ok((Err(message), _)) => {
                let _ = app_thread.emit(
                    "translation://error",
                    ErrorPayload { task_id: tid.clone(), message: message.to_string() },
                );
            }
            Err(_) => {
                let _ = app_thread.emit(
                    "translation://error",
                    ErrorPayload { task_id: tid.clone(), message: "translation task panicked".into() },
                );
            }
        }
    });

    Ok(Some(task_id))
}

/// Relay pipeline progress events to the frontend (`translation://progress`).
async fn forward_translation_events(
    app: &AppHandle,
    task_id: &str,
    mut rx: mpsc::UnboundedReceiver<PipelineEvent>,
) {
    while let Some(event) = rx.recv().await {
        let _ = app.emit(
            "translation://progress",
            TranslationProgressPayload { task_id: task_id.to_string(), event },
        );
    }
}

/// Request a stop of the active translation run (graceful, or aborting per the
/// project's `stop_aborts_inflight` setting). The run thread ends and clears
/// itself; a `translation://done` / `translation://error` event follows.
#[tauri::command]
pub fn stop_translation(state: State<'_, AppState>) -> Result<(), String> {
    let guard = state.translation_guard();
    match guard.as_ref() {
        Some(run) => {
            let _ = run.stop.send(true);
            Ok(())
        }
        None => Err("no translation is running".into()),
    }
}

/// Live view of the pipeline: whether a run is active, the project's N/W, the
/// current activation window (chapter ids), and TU counts by status.
#[tauri::command]
pub fn translation_status(state: State<'_, AppState>) -> Result<TranslationStatusView, String> {
    let (running, task_id) = {
        let guard = state.translation_guard();
        (guard.is_some(), guard.as_ref().map(|r| r.task_id.clone()))
    };
    with_project(&state, |p| {
        let settings = p.db.get_translation_settings()?;
        let active_chapters = p.db.active_chapter_ids(settings.window as usize)?;
        let counts = p
            .db
            .counts_by_status()?
            .into_iter()
            .map(|(status, count)| StatusCount { status: status.as_str().to_string(), count })
            .collect();
        Ok(TranslationStatusView {
            running,
            task_id,
            workers: settings.workers,
            window: settings.window,
            active_chapters,
            counts,
        })
    })
}

/// Explicit retry: re-queue `failed_*`/`interrupted` TUs, scoped to
/// `scope` = `"tu"` (ids), `"chapter"` (ids[0]), or `"all"`. Returns how many
/// were re-queued. If a run is active its scheduler is woken; if the pipeline
/// is idle a run is started so the requeued TUs actually proceed.
#[tauri::command]
pub fn retry_translation(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    scope: String,
    ids: Vec<i64>,
) -> Result<usize, String> {
    let requeued = with_project(&state, |p| match scope.as_str() {
        "tu" => p.db.requeue_failed(Some(&ids), None),
        "chapter" => {
            let ch = *ids.first().ok_or_else(|| felin_core::Error::InvalidInput {
                detail: "scope=chapter requires a chapter id".into(),
            })?;
            p.db.requeue_failed(None, Some(ch))
        }
        "all" => p.db.requeue_failed(None, None),
        other => Err(felin_core::Error::InvalidInput {
            detail: format!("unknown retry scope: {other}"),
        }),
    })?;
    if requeued > 0 {
        // Wake an active run, or start one if the pipeline is idle (else the
        // requeued TUs would sit `queued` forever with nothing to pick them up).
        let run_active = state.translation_guard().is_some();
        if run_active {
            if let Some(run) = state.translation_guard().as_ref() {
                run.wake.notify_one();
            }
        } else {
            let _ = spawn_translation(&app, &state);
        }
    }
    Ok(requeued)
}

/// Human "approve" transition: `translated`/`reviewing` → `approved`.
#[tauri::command]
pub fn approve_tu(state: State<'_, AppState>, tu_id: i64) -> Result<bool, String> {
    with_project(&state, |p| p.db.approve_tu(tu_id))
}

/// Persist (or clear, when empty) a per-item translation instruction.
#[tauri::command]
pub fn set_tu_instruction(
    state: State<'_, AppState>,
    tu_id: i64,
    instruction: String,
) -> Result<(), String> {
    with_project(&state, |p| p.db.set_tu_instruction(tu_id, &instruction))
}

/// Re-translate one TU: move it back to `queued` (with optional per-item
/// instruction) and mark it for a fresh LLM call. Returns false if the TU is
/// mid-flight (`translating`). If a run is active its scheduler is woken; if
/// the pipeline is idle a run is started so the requeued TU actually proceeds.
#[tauri::command]
pub fn retranslate_tu(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    tu_id: i64,
    instruction: String,
) -> Result<bool, String> {
    let ok = with_project(&state, |p| p.db.retranslate_tu(tu_id, &instruction))?;
    if ok {
        if state.translation_guard().is_some() {
            if let Some(run) = state.translation_guard().as_ref() {
                run.wake.notify_one();
            }
        } else {
            let _ = spawn_translation(&app, &state);
        }
    }
    Ok(ok)
}

#[tauri::command]
pub fn get_translation_settings(state: State<'_, AppState>) -> Result<TranslationSettings, String> {
    with_project(&state, |p| p.db.get_translation_settings())
}

#[tauri::command]
pub fn set_translation_settings(
    state: State<'_, AppState>,
    settings: TranslationSettings,
) -> Result<(), String> {
    with_project(&state, |p| p.db.set_translation_settings(&settings))
}

/// The project 总则 (system prompt), falling back to the default template.
#[tauri::command]
pub fn get_guidelines(state: State<'_, AppState>) -> Result<String, String> {
    with_project(&state, |p| p.db.get_guidelines())
}

#[tauri::command]
pub fn set_guidelines(state: State<'_, AppState>, text: String) -> Result<(), String> {
    with_project(&state, |p| p.db.set_guidelines(&text))
}

/// The runtime-effective `[prompt]` templates from felin.toml (empty fields are
/// returned verbatim — an empty string means that message section isn't sent).
/// App-level: no project needs to be open.
#[tauri::command]
pub fn get_prompt_config(state: State<'_, AppState>) -> Result<PromptConfig, String> {
    Ok(state.prompt_config().clone())
}

/// Write the `[prompt]` section back to `<data_dir>/felin.toml` (other
/// sections/comments preserved) and apply it to the live config immediately —
/// the very next translation / name-extraction run uses the new text, no
/// restart required. Returns a clear error if felin.toml cannot be written.
#[tauri::command]
pub fn set_prompt_config(state: State<'_, AppState>, config: PromptConfig) -> Result<(), String> {
    let path = state.data_dir.join("felin.toml");
    felin_core::config::set_prompt_section(&path, &config)?;
    *state.prompt_config() = config;
    Ok(())
}

/// A TU joined with its translation row — the read-only status list the review
/// screen drives from (step 8's minimal UI).
#[tauri::command]
pub fn list_tus_with_translations(
    state: State<'_, AppState>,
    chapter_id: i64,
) -> Result<Vec<TuWithTranslation>, String> {
    with_project(&state, |p| p.db.list_tus_with_translations(chapter_id))
}

/// Override the effective source text of a TU (what translation/editing sees);
/// passing a blank string clears the override and falls back to the paragraphs.
#[tauri::command]
pub fn set_tu_source(
    state: State<'_, AppState>,
    tu_id: i64,
    source: String,
) -> Result<(), String> {
    with_project(&state, |p| p.db.set_tu_source(tu_id, &source))
}

/// Split a TU into two TUs at the given UTF-16 text offset (the 原文 TextArea's
/// caret), either at a paragraph boundary or mid-paragraph. The original TU
/// keeps its id (demoted, its draft/llm text cleared) and a new `pending` TU is
/// inserted right after it. Only valid when the TU's source is its raw
/// paragraphs (no `source_override`) and it is not mid-flight.
#[tauri::command]
pub fn split_tu_paragraph(
    state: State<'_, AppState>,
    tu_id: i64,
    offset: usize,
) -> Result<(), String> {
    with_project(&state, |p| p.db.split_tu_at(tu_id, offset))
}

/// Persist an edited translation. Returns true if this demoted an
/// approved/exported TU back to reviewing (i.e. the TU is no longer final).
#[tauri::command]
pub fn set_translation_text(
    state: State<'_, AppState>,
    tu_id: i64,
    text: String,
) -> Result<bool, String> {
    with_project(&state, |p| p.db.set_translation_text(tu_id, &text))
}

/// Batch-delete TUs — any status, including `translating`/`approved`/`exported`
/// (the user deletes 不需要/识别错误的段 outright). Each TU's translation row is
/// removed (FK cascade), and its paragraphs are removed when no other TU still
/// references them (shared paragraphs are kept). The frontend re-pulls the list
/// afterwards. Returns how many TUs were deleted.
#[tauri::command]
pub fn delete_tus(state: State<'_, AppState>, ids: Vec<i64>) -> Result<usize, String> {
    with_project(&state, |p| p.db.delete_tus(&ids))
}

/// Re-translate a batch of TUs: requeue each (with an optional per-run
/// instruction) and mark them for a fresh LLM call. If a run is active its
/// scheduler is woken; if the pipeline is idle a run is started so the requeued
/// TUs actually proceed. Returns how many were requeued (mid-flight `translating`
/// TUs are the only exclusion; the count therefore matches what the user
/// selected).
#[tauri::command]
pub fn retranslate_tus(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    ids: Vec<i64>,
    instruction: Option<String>,
) -> Result<usize, String> {
    let n = with_project(&state, |p| p.db.retranslate_tus(&ids, instruction.as_deref()))?;
    if n > 0 {
        if state.translation_guard().is_some() {
            if let Some(run) = state.translation_guard().as_ref() {
                run.wake.notify_one();
            }
        } else {
            let _ = spawn_translation(&app, &state);
        }
    }
    Ok(n)
}

/// GUI-managed OCR options (batch worker count, recursion) for this project.
#[tauri::command]
pub fn get_ocr_settings(state: State<'_, AppState>) -> Result<OcrSettings, String> {
    with_project(&state, |p| p.db.get_ocr_settings())
}

#[tauri::command]
pub fn set_ocr_settings(
    state: State<'_, AppState>,
    settings: OcrSettings,
) -> Result<(), String> {
    with_project(&state, |p| p.db.set_ocr_settings(&settings))
}

/// Read the app-editable slice of the ocr-router `config.yaml` — the file
/// referenced by felin.toml `[sidecar] config` / `FELIN_SIDECAR_CONFIG`. No
/// project needs to be open; the config is app-level. Errors if the path was
/// never configured or the file is missing ("禁止硬编码，找不到即报错").
#[tauri::command]
pub fn get_ocr_config(state: State<'_, AppState>) -> Result<OcrConfig, String> {
    let path = state
        .sidecar_config
        .as_ref()
        .ok_or_else(|| "未配置 OCR 配置文件（felin.toml [sidecar] config 或 FELIN_SIDECAR_CONFIG）".to_string())?;
    read_config_file(path).map_err(|e| e.to_string())
}

/// Write the app-editable slice back to the **same** config.yaml, in place.
/// Unmanaged sections and `${ENV}` placeholders are preserved; comments and
/// hand formatting are normalized (the UI warns the user about this).
#[tauri::command]
pub fn set_ocr_config(state: State<'_, AppState>, config: OcrConfig) -> Result<(), String> {
    let path = state
        .sidecar_config
        .as_ref()
        .ok_or_else(|| "未配置 OCR 配置文件（felin.toml [sidecar] config 或 FELIN_SIDECAR_CONFIG）".to_string())?;
    if !path.exists() {
        return Err(format!("OCR 配置文件不存在：{}", path.display()));
    }
    apply_and_write(path, &config).map_err(|e| e.to_string())
}

/// Deterministic 译文导出 into `dest_dir`: a 汉化 .txt and a 译文.csv, both
/// recorded in the project's `exports` table.
#[tauri::command]
pub fn export_translations(
    state: State<'_, AppState>,
    dest_dir: String,
) -> Result<TranslationExport, String> {
    let out = with_project(&state, |p| p.db.export_translations(Path::new(&dest_dir)))?;
    tracing::debug!(dest_dir, tus = out.tus, "translation export finished");
    Ok(out)
}

// ----- project small glossary (self-contained, travels with the archive) ----

/// List the project's small-glossary entries, optionally filtered by a
/// free-text query (matches japanese/chinese/english/tags).
#[tauri::command]
pub fn list_glossary_entries(
    state: State<'_, AppState>,
    q: Option<String>,
) -> Result<Vec<GlossaryEntry>, String> {
    with_project(&state, |p| p.db.list_glossary_entries(q.as_deref()))
}

/// Add an entry to the project's small glossary (upsert by japanese). When
/// `name_global_id` is given (a "from global search add"), provenance is
/// recorded so the project archive stays self-contained.
#[tauri::command]
pub fn add_glossary_entry(
    state: State<'_, AppState>,
    name_global_id: Option<i64>,
    japanese: String,
    chinese: Option<String>,
    english: Option<String>,
    category: Option<String>,
    tags: Vec<String>,
    notes: Option<String>,
) -> Result<i64, String> {
    let guard = state.project_guard();
    let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
    if let Some(gid) = name_global_id {
        state
            .global
            .get_name(gid)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "global entry not found".to_string())?;
    }
    proj.db
        .insert_glossary_entry(
            name_global_id,
            &japanese,
            chinese.as_deref(),
            english.as_deref(),
            category.as_deref(),
            &tags,
            notes.as_deref(),
        )
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_glossary_entry(
    state: State<'_, AppState>,
    id: i64,
    japanese: String,
    chinese: Option<String>,
    english: Option<String>,
    category: Option<String>,
    tags: Vec<String>,
    notes: Option<String>,
) -> Result<(), String> {
    with_project(&state, |p| {
        p.db.update_glossary_entry(
            id,
            &japanese,
            chinese.as_deref(),
            english.as_deref(),
            category.as_deref(),
            &tags,
            notes.as_deref(),
        )
    })
}

/// Toggle whether an entry is injected into translation prompts.
#[tauri::command]
pub fn set_entry_enabled(
    state: State<'_, AppState>,
    id: i64,
    enabled: bool,
) -> Result<(), String> {
    with_project(&state, |p| p.db.set_entry_enabled(id, enabled))
}

/// Replace an entry's tag array wholesale.
#[tauri::command]
pub fn set_entry_tags(
    state: State<'_, AppState>,
    id: i64,
    tags: Vec<String>,
) -> Result<(), String> {
    with_project(&state, |p| p.db.set_entry_tags(id, &tags))
}

#[tauri::command]
pub fn delete_glossary_entry(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    with_project(&state, |p| p.db.delete_glossary_entry(id))
}

/// Batch-delete entries from the project small glossary. Returns how many were
/// deleted.
#[tauri::command]
pub fn delete_glossary_entries(state: State<'_, AppState>, ids: Vec<i64>) -> Result<usize, String> {
    with_project(&state, |p| p.db.delete_glossary_entries(&ids))
}

// ----- global big glossary (shared pool, tag/enabled managed here) ----------

/// List global entries; `q` filters by japanese/chinese/english/tag, empty
/// lists everything (most-recently-updated first), capped at `limit`.
#[tauri::command]
pub fn list_glossary(
    state: State<'_, AppState>,
    q: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<GlossaryName>, String> {
    let limit = limit.unwrap_or(500);
    match q.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(q) => state.global.search_names(&q, limit).map_err(|e| e.to_string()),
        None => state.global.list_names(limit).map_err(|e| e.to_string()),
    }
}

#[tauri::command]
pub fn set_global_name_tags(
    state: State<'_, AppState>,
    id: i64,
    tags: Vec<String>,
) -> Result<(), String> {
    state.global.set_name_tags(id, &tags).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_global_name_enabled(
    state: State<'_, AppState>,
    id: i64,
    enabled: bool,
) -> Result<(), String> {
    state.global.set_name_enabled(id, enabled).map_err(|e| e.to_string())
}

/// Batch-delete global big-glossary entries (`name_history` cascades via FK).
/// Project small-glossary entries that carry `name_global_id` provenance for a
/// deleted name are kept, with the pointer cleared on the currently open project
/// so no dangling cross-file reference survives. Returns how many global entries
/// were deleted.
#[tauri::command]
pub fn delete_global_names(state: State<'_, AppState>, ids: Vec<i64>) -> Result<usize, String> {
    let n = state.global.delete_names(&ids).map_err(|e| e.to_string())?;
    let guard = state.project_guard();
    if let Some(p) = guard.as_ref() {
        p.db.clear_global_provenance(&ids).map_err(|e| e.to_string())?;
    }
    Ok(n)
}

/// Round-trip one tiny translation through the configured model — the Settings
/// page's "测试连通" button.
#[tauri::command]
pub async fn test_llm_connection(state: State<'_, AppState>) -> Result<(), String> {
    let cfg = {
        let guard = state.project_guard();
        let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
        load_llm_config(&proj.db, &state.config.llm)?
    };
    let client = LlmClient::with_limiter(cfg, Arc::clone(&state.llm_limiter)).map_err(|e| e.to_string())?;
    let req = TranslateRequest {
        guidelines: "你是连接测试助手。请只回复两个字：通过。".into(),
        source: "连接测试".into(),
        ..Default::default()
    };
    client.translate(&req).await.map_err(|e| format!("LLM 连接测试失败：{e}"))?;
    Ok(())
}


/// (Re-)segment the open project: clean text, detect chapters, rebuild TUs.
/// `block_size` is the soft target block size (characters); when given it is
/// saved on the project and reused next time, else the saved/default value is used.
#[tauri::command]
pub fn segment_project(state: State<'_, AppState>, budget: Option<i64>) -> Result<SegmentResult, String> {
    let seg_cfg = &state.config.seg;
    let recognizer = felin_core::seg::ChapterRecognizer::new(
        &seg_cfg.chapter_heading_patterns,
        seg_cfg.heading_max_chars,
    );
    let fallback = seg_cfg.fallback_chapter_title.clone();
    let default_block = seg_cfg.default_block_size;
    with_project(&state, |p| {
        let block = match budget.filter(|b| *b > 0) {
            Some(b) => {
                p.db.set_setting("tu_block_size", &b.to_string())?;
                b as usize
            }
            None => p
                .db
                .get_setting("tu_block_size")?
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|b| *b > 0)
                .unwrap_or(default_block),
        };
        let out = p.db.segment(block, &fallback, &recognizer)?;
        tracing::debug!(chapters = out.chapters, tus = out.tus, block_size = block, "segmentation complete");
        Ok(SegmentResult { chapters: out.chapters, tus: out.tus })
    })
}

// PLACEHOLDER_IMPORTS

/// Import a plain-text file into a chapter named after the file (encoding is
/// auto-detected). Reads the source in place; nothing is copied.
#[tauri::command]
pub fn import_txt_file(state: State<'_, AppState>, path: String) -> Result<TxtImportResult, String> {
    let bytes = read_regular_capped(Path::new(&path), state.config.import.max_file_bytes)?;
    let stem = Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("import").to_string();
    let paras = felin_core::ocr::txt::import_txt(&bytes, &stem).map_err(|e| e.to_string())?;
    with_project(&state, |p| {
        let ch = p.db.get_or_create_chapter(&stem)?;
        let n = p.db.insert_paragraphs(ch, &paras)?;
        Ok(TxtImportResult { chapter_id: ch, paragraphs: n })
    })
}

/// Start an OCR import of `input` (read in place). Returns a `task_id`
/// immediately; progress arrives via `ocr://progress` and completion via
/// `ocr://done` / `ocr://error`. Cancel with [`cancel_import`].
#[tauri::command]
pub fn import_ocr(
    app: AppHandle,
    state: State<'_, AppState>,
    input: String,
    pages: Option<String>,
) -> Result<String, String> {
    // Fast-fail synchronously so the error is the invoke rejection (before any
    // task_id / events exist), not a phantom event the frontend can't match.
    let sidecar = state.sidecar.clone().ok_or_else(|| {
        "OCR sidecar not configured: set [sidecar] bin in felin.toml or FELIN_SIDECAR to the ocr-cli binary"
            .to_string()
    })?;
    if !sidecar.exists() {
        return Err(format!("OCR sidecar not found at {}", sidecar.display()));
    }
    // `extract` handles a single PDF or image file; an image *folder* goes
    // through the batch flow (rule matching + scan preview + staging).
    let input_path = Path::new(&input);
    if input_path.is_dir() {
        return Err(
            "input is a directory: use 图片目录导入 (import_images_batch) for image folders; \
             extract handles a single PDF or image file"
                .to_string(),
        );
    }
    // Capture a handle to *this* project (db + root together, under one lock) so
    // ingest targets it even if the user switches/closes the open project mid-import.
    let (db, root) = {
        let guard = state.project_guard();
        let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
        (Arc::clone(&proj.db), proj.root.clone())
    };
    let threshold = state.config.ocr.low_score_threshold;
    let enders = state.config.sentence_enders();
    let max_page = state.config.ocr.max_page_json_bytes;
    let max_manifest = state.config.ocr.max_manifest_bytes;

    let task_id = uuid::Uuid::new_v4().to_string();
    let stem = Path::new(&input).file_stem().and_then(|s| s.to_str()).unwrap_or("import").to_string();
    // Namespace the OCR output by task id so concurrent/repeat imports never
    // collide on the same directory or manifest.
    let out_dir = root.join("ocr").join(&task_id);
    let manifest = out_dir.join(format!("{stem}.manifest.json"));

    let (tx, rx) = watch::channel(false);
    state.tasks_guard().insert(task_id.clone(), tx);

    // Pass the user-configured sidecar config (felin.toml / env) when present.
    // A config that was configured but is missing is a hard error (found-but-
    // silently-dropped would make ocr-cli fall back to its own default and die
    // with a cryptic exit 20); `None` means "let ocr-cli find its own config".
    let config = match &state.sidecar_config {
        Some(p) if !p.exists() => {
            return Err(format!(
                "OCR sidecar config set at {} but not found",
                p.display()
            ))
        }
        other => other.clone(),
    };

    let args = ExtractArgs {
        sidecar,
        input: PathBuf::from(&input),
        config,
        out_dir,
        manifest,
        pages,
        page_workers: None,
        skip_existing: false,
        extra: vec![],
        envs: vec![],
    };

    let app_thread = app.clone();
    let tid = task_id.clone();
    std::thread::spawn(move || {
        // RAII: always deregister the cancel handle, on every exit path.
        struct Dereg {
            app: AppHandle,
            tid: String,
        }
        impl Drop for Dereg {
            fn drop(&mut self) {
                if let Some(st) = self.app.try_state::<AppState>() {
                    st.tasks_guard().remove(&self.tid);
                }
            }
        }
        let _dereg = Dereg { app: app_thread.clone(), tid: tid.clone() };

        let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = app_thread.emit(
                    "ocr://error",
                    ErrorPayload { task_id: tid, message: format!("could not start runtime: {e}") },
                );
                return;
            }
        };
        // catch_unwind so a panic still produces a terminal event.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rt.block_on(run_import(&app_thread, &tid, &args, db, stem, threshold, enders, max_page, max_manifest, rx))
        }));
        match result {
            Ok(Ok(r)) => {
                let _ = app_thread.emit("ocr://done", r);
            }
            Ok(Err(message)) => {
                let _ = app_thread.emit("ocr://error", ErrorPayload { task_id: tid.clone(), message });
            }
            Err(_) => {
                let _ = app_thread.emit(
                    "ocr://error",
                    ErrorPayload { task_id: tid.clone(), message: "import task panicked".into() },
                );
            }
        }
    });

    Ok(task_id)
}

/// Async body of an OCR import: spawn+stream the sidecar, reconcile the
/// manifest, and ingest into the captured project DB (`db`).
async fn run_import(
    app: &AppHandle,
    task_id: &str,
    args: &ExtractArgs,
    db: Arc<ProjectDb>,
    stem: String,
    low_score_threshold: f64,
    sentence_enders: Vec<char>,
    max_page_json_bytes: u64,
    max_manifest_bytes: u64,
    rx: watch::Receiver<bool>,
) -> Result<ImportResult, String> {
    tracing::debug!(task_id, input = %args.input.display(), "OCR import started");
    let app_prog = app.clone();
    let tid = task_id.to_string();
    let outcome = run_extract(
        args,
        move |event| {
            let _ = app_prog.emit("ocr://progress", ProgressPayload { task_id: tid.clone(), event });
        },
        rx,
    )
    .await
    .map_err(|e| e.to_string())?;

    let manifest = read_manifest(&args.manifest, max_manifest_bytes).map_err(|e| e.to_string())?;
    let res = ingest_from_manifest(
        &args.out_dir,
        &manifest,
        low_score_threshold,
        false,
        &sentence_enders,
        max_page_json_bytes,
    )
    .map_err(|e| e.to_string())?;

    // Ingest into the captured project (no global state lock held here).
    let ch = db.get_or_create_chapter(&stem).map_err(|e| e.to_string())?;
    db.insert_paragraphs(ch, &res.paragraphs).map_err(|e| e.to_string())?;

    tracing::debug!(
        task_id,
        outcome = outcome_str(outcome),
        pages_ok = res.pages_ok,
        pages_failed = res.failed_pages.len(),
        paragraphs = res.paragraphs.len(),
        "OCR import finished"
    );

    Ok(ImportResult {
        task_id: task_id.to_string(),
        outcome: outcome_str(outcome),
        pages_ok: res.pages_ok,
        pages_failed: res.failed_pages.len(),
        failed_pages: res.failed_pages,
        paragraphs: res.paragraphs.len(),
        chapter_id: ch,
    })
}

/// Request cancellation of an in-flight OCR import.
#[tauri::command]
pub fn cancel_import(state: State<'_, AppState>, task_id: String) -> Result<(), String> {
    let tasks = state.tasks_guard();
    match tasks.get(&task_id) {
        Some(tx) => {
            let _ = tx.send(true);
            Ok(())
        }
        None => Err(format!("no active import task '{task_id}'")),
    }
}

// ----- image-directory import (ocr-cli batch, app-side selection) -----------

/// Preview which images in `dir` match `rule`, in natural reading order —
/// what the import card shows before the user commits to the batch run.
#[tauri::command]
pub fn scan_image_dir(dir: String, rule: ImageMatchRule) -> Result<FileSelection, String> {
    let dir = PathBuf::from(&dir);
    let matched = select_images(&dir, &rule).map_err(|e| e.to_string())?;
    // The default rule (preset All, no glob/regex/range) = every image file.
    let total = select_images(&dir, &ImageMatchRule::default())
        .map_err(|e| e.to_string())?
        .len();
    let names = matched
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    let bytes = matched.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
    Ok(FileSelection { total, matched: matched.len(), names, bytes })
}

/// Stage the selected images into `staged_dir` under their original names so
/// `batch` only ever sees the *selected* files — a PDF mixed into an image
/// folder is never staged and never processed (non-expected input = skip).
/// Unix prefers a symlink; falls back to a hard link, then a byte copy.
fn stage_images(files: &[PathBuf], staged_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(staged_dir).map_err(|e| e.to_string())?;
    for f in files {
        let name = f
            .file_name()
            .ok_or_else(|| format!("cannot derive file name from {}", f.display()))?;
        let target = staged_dir.join(name);
        #[cfg(unix)]
        if std::os::unix::fs::symlink(f, &target).is_ok() {
            continue;
        }
        if std::fs::hard_link(f, &target).is_ok() {
            continue;
        }
        std::fs::copy(f, &target).map_err(|e| format!("cannot stage {}: {e}", f.display()))?;
    }
    Ok(())
}

/// Removes a directory on drop — used to guarantee the OCR batch staging dir
/// (`inputs/`, symlinks to the user's source images) is cleaned up on every
/// exit path, never left behind.
struct CleanupDir(PathBuf);
impl Drop for CleanupDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Import the images in `dir` that match `rule` via the `ocr-cli batch`
/// sidecar. Returns a `task_id` immediately; progress arrives over the same
/// `ocr://progress` / `ocr://done` / `ocr://error` events as [`import_ocr`],
/// and the import can be cancelled with [`cancel_import`]. The images are
/// staged into `<project>/ocr/<task>/inputs/` (symlinks), the txt/json
/// outputs land in `<project>/ocr/<task>/`, paragraphs are ingested into a
/// chapter named after the directory, and the staging dir is removed.
#[tauri::command]
pub fn import_images_batch(
    app: AppHandle,
    state: State<'_, AppState>,
    dir: String,
    rule: ImageMatchRule,
) -> Result<String, String> {
    let sidecar = state.sidecar.clone().ok_or_else(|| {
        "OCR sidecar not configured: set [sidecar] bin in felin.toml or FELIN_SIDECAR to the ocr-cli binary"
            .to_string()
    })?;
    if !sidecar.exists() {
        return Err(format!("OCR sidecar not found at {}", sidecar.display()));
    }
    let dir_path = PathBuf::from(&dir);
    if !dir_path.is_dir() {
        return Err(format!("not a directory: {}", dir_path.display()));
    }
    // Selection happens app-side (batch has no pattern/range flag): only the
    // matched images are staged, so a mixed-in PDF is skipped outright.
    let files = select_images(&dir_path, &rule).map_err(|e| e.to_string())?;
    if files.is_empty() {
        return Err("no images in the directory matched the selection rule".into());
    }
    // Capture everything the run thread needs under the project lock.
    let (db, root, chapter_title, ocr_settings) = {
        let guard = state.project_guard();
        let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
        let title = dir_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "图片导入".to_string());
        let settings = proj.db.get_ocr_settings().map_err(|e| e.to_string())?;
        (Arc::clone(&proj.db), proj.root.clone(), title, settings)
    };

    let task_id = uuid::Uuid::new_v4().to_string();
    let out_dir = root.join("ocr").join(&task_id);
    let staged = out_dir.join("inputs");
    if let Err(e) = stage_images(&files, &staged) {
        let _ = std::fs::remove_dir_all(&out_dir);
        return Err(e);
    }

    let config = match &state.sidecar_config {
        Some(p) if !p.exists() => {
            return Err(format!(
                "OCR sidecar config set at {} but not found",
                p.display()
            ))
        }
        other => other.clone(),
    };
    let args = BatchArgs {
        sidecar,
        input_dir: staged,
        config,
        out_dir: out_dir.clone(),
        workers: Some(ocr_settings.batch_workers.clamp(1, 16) as u32),
        recursive: ocr_settings.batch_recursive,
        skip_existing: false,
        save_json: true,
        envs: vec![],
    };

    let (tx, rx) = watch::channel(false);
    state.tasks_guard().insert(task_id.clone(), tx);
    let threshold = state.config.ocr.low_score_threshold;
    let enders = state.config.sentence_enders();

    let app_thread = app.clone();
    let tid = task_id.clone();
    std::thread::spawn(move || {
        // RAII: always deregister the cancel handle, on every exit path.
        struct Dereg {
            app: AppHandle,
            tid: String,
        }
        impl Drop for Dereg {
            fn drop(&mut self) {
                if let Some(st) = self.app.try_state::<AppState>() {
                    st.tasks_guard().remove(&self.tid);
                }
            }
        }
        let _dereg = Dereg { app: app_thread.clone(), tid: tid.clone() };

        let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                let _ = app_thread.emit(
                    "ocr://error",
                    ErrorPayload { task_id: tid, message: format!("could not start runtime: {e}") },
                );
                return;
            }
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rt.block_on(run_batch_import(
                &app_thread,
                &tid,
                &args,
                files,
                db,
                chapter_title,
                threshold,
                enders,
                rx,
            ))
        }));
        match result {
            Ok(Ok(r)) => {
                let _ = app_thread.emit("ocr://done", r);
            }
            Ok(Err(message)) => {
                let _ = app_thread.emit("ocr://error", ErrorPayload { task_id: tid.clone(), message });
            }
            Err(_) => {
                let _ = app_thread.emit(
                    "ocr://error",
                    ErrorPayload { task_id: tid.clone(), message: "import task panicked".into() },
                );
            }
        }
    });

    Ok(task_id)
}

/// Async body of an image-directory import: stream the batch sidecar, ingest
/// the per-image txt outputs (with score metadata) into the captured project,
/// then remove the staging directory.
async fn run_batch_import(
    app: &AppHandle,
    task_id: &str,
    args: &BatchArgs,
    files: Vec<PathBuf>,
    db: Arc<ProjectDb>,
    chapter_title: String,
    low_score_threshold: f64,
    sentence_enders: Vec<char>,
    rx: watch::Receiver<bool>,
) -> Result<ImportResult, String> {
    let total = files.len() as i64;
    let app_prog = app.clone();
    let tid = task_id.to_string();
    let _ = app_prog.emit(
        "ocr://progress",
        ProgressPayload {
            task_id: tid.clone(),
            event: ProgressEvent::Start { source: chapter_title.clone(), pages_total: total },
        },
    );

    let mut ok: usize = 0;
    let mut failed: usize = 0;
    let mut failed_pages: Vec<i64> = Vec::new();
    // RAII: the batch staging dir (`inputs/`, symlinks to the user's source
    // images) is transient — remove it on every exit path (success, error,
    // cancellation, panic), never keep the source inputs around.
    let _inputs_guard = CleanupDir(args.input_dir.clone());
    // A clone of the cancel flag outlives `run_batch` (which consumes `rx`),
    // so after the run we can tell a cancellation from normal completion.
    let cancel_probe = rx.clone();
    run_batch(
        args,
        |ev| {
            let done = ok + failed + 1;
            let (status, error) = match ev {
                BatchEvent::Done { .. } => {
                    ok += 1;
                    (PageStatus::Ok, None)
                }
                BatchEvent::Failed { error, .. } => {
                    failed += 1;
                    failed_pages.push(done as i64);
                    (PageStatus::Failed, Some(error))
                }
            };
            let _ = app_prog.emit(
                "ocr://progress",
                ProgressPayload {
                    task_id: tid.clone(),
                    event: ProgressEvent::Page {
                        page: done as i64,
                        status,
                        score: None,
                        error,
                        done: done as i64,
                        total,
                    },
                },
            );
        },
        rx,
    )
    .await
    .map_err(|e| e.to_string())?;
    // `run_batch` returns Ok on both normal completion and cancellation; the
    // watch value distinguishes the two.
    let cancelled = *cancel_probe.borrow();
    let outcome = if cancelled {
        "cancelled"
    } else if failed > 0 {
        "partial"
    } else {
        "all_ok"
    };
    let _ = app_prog.emit(
        "ocr://progress",
        ProgressPayload {
            task_id: tid.clone(),
            event: ProgressEvent::Done { pages_ok: ok as i64, pages_failed: failed as i64, manifest: None },
        },
    );

    // Each matched image produced `<out>/<stem>.txt`; assemble paragraphs (a
    // failed image has no txt and is skipped by the ingest).
    let paras =
        ingest_batch_txts(&files, &args.out_dir, low_score_threshold, &sentence_enders)
            .map_err(|e| e.to_string())?;
    let ch = db.get_or_create_chapter(&chapter_title).map_err(|e| e.to_string())?;
    db.insert_paragraphs(ch, &paras).map_err(|e| e.to_string())?;

    // The staging inputs were only for `batch`; remove them now that the txts
    // are ingested (the txt/json outputs in `out_dir` are kept like `extract`).
    // `CleanupDir` also covers the error/cancel/panic exit paths.
    let _ = std::fs::remove_dir_all(&args.input_dir);

    Ok(ImportResult {
        task_id: task_id.to_string(),
        outcome: outcome.to_string(),
        pages_ok: ok,
        pages_failed: failed,
        failed_pages,
        paragraphs: paras.len(),
        chapter_id: ch,
    })
}

// PLACEHOLDER_ARCHIVE

/// Export the currently-open project to a single compressed archive at
/// `dest_path` (a location the user picks — e.g. next to their source files),
/// with a SHA-256 digest for integrity. Lets a project be moved/backed up even
/// though its data normally lives next to the software.
///
/// Returns a `task_id` immediately; the pack runs on a background thread so the
/// UI isn't blocked. Progress arrives via `export://progress`, completion via
/// `export://done`, and failures via `export://error` (same pattern as OCR
/// import).
#[tauri::command]
pub fn export_project(app: AppHandle, state: State<'_, AppState>, dest_path: String) -> Result<ExportResult, String> {
    let (root, slug) = {
        let guard = state.project_guard();
        let p = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
        (p.root.clone(), p.slug.clone())
    };
    let dest = PathBuf::from(&dest_path);

    let task_id = uuid::Uuid::new_v4().to_string();
    let app_thread = app.clone();
    let tid = task_id.clone();
    let tid_prog = task_id.clone();
    let dest_thread = dest.clone();
    let dest_display = dest.display().to_string();
    let dest_display_ret = dest_display.clone();
    std::thread::spawn(move || {
        // The pack is blocking (compress + hash); run it on a plain OS thread.
        // No dedicated runtime needed — no async I/O happens here.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            archive::export_project(&root, &slug, &dest_thread, Some(|ev| {
                let _ = app_thread.emit(
                    "export://progress",
                    ExportProgressPayload { task_id: tid_prog.clone(), event: ev },
                );
            }))
        }));
        match result {
            Ok(Ok(out)) => {
                let _ = app_thread.emit(
                    "export://done",
                    ExportResult {
                        task_id: tid.clone(),
                        archive: dest_display.clone(),
                        sha256: out.sha256,
                        bytes: out.bytes,
                        files: out.files,
                    },
                );
            }
            Ok(Err(e)) => {
                let _ = app_thread.emit(
                    "export://error",
                    ErrorPayload { task_id: tid.clone(), message: e.to_string() },
                );
            }
            Err(_) => {
                let _ = app_thread.emit(
                    "export://error",
                    ErrorPayload { task_id: tid.clone(), message: "export task panicked".into() },
                );
            }
        }
    });

    Ok(ExportResult { task_id, archive: dest_display_ret, sha256: String::new(), bytes: 0, files: 0 })
}

/// Import a project archive into the app-side projects dir (verifying every
/// file's checksum against the embedded manifest), returning its summary.
#[tauri::command]
pub fn import_project(
    state: State<'_, AppState>,
    archive_path: String,
) -> Result<ProjectSummary, String> {
    let projects = state.projects_dir();
    let slug =
        archive::import_project(Path::new(&archive_path), &projects).map_err(|e| e.to_string())?;
    let summary: ProjectSummary =
        serde_json::from_slice(&std::fs::read(meta_path(&projects.join(&slug))).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    Ok(summary)
}

// ---------------------------------------------------------------------------
// Glossary / proper-noun names
// ---------------------------------------------------------------------------

/// CSV column mapping from the frontend.
#[derive(Deserialize)]
pub struct CsvMapping {
    pub japanese: usize,
    pub chinese: usize,
    pub english: Option<usize>,
    pub category: Option<usize>,
    pub notes: Option<usize>,
    pub has_header: bool,
}

impl CsvMapping {
    fn into_core(self) -> names::ColumnMapping {
        names::ColumnMapping {
            japanese: self.japanese,
            chinese: self.chinese,
            english: self.english,
            category: self.category,
            notes: self.notes,
            has_header: self.has_header,
        }
    }
}

#[derive(Serialize, Clone)]
pub struct LlmConfigView {
    pub endpoint: String,
    pub model: String,
    pub has_key: bool,
}

/// Build an `LlmConfig` from the technical defaults plus the project's saved
/// user-facing settings (endpoint/model/key).
fn load_llm_config(
    db: &ProjectDb,
    defaults: &felin_core::config::LlmDefaults,
) -> Result<felin_core::llm::LlmConfig, String> {
    let mut cfg = felin_core::llm::LlmConfig {
        timeout: std::time::Duration::from_secs(defaults.timeout_secs),
        max_retries: defaults.max_retries,
        base_delay: std::time::Duration::from_millis(defaults.base_delay_ms),
        max_delay: std::time::Duration::from_secs(defaults.max_delay_secs),
        temperature: defaults.temperature,
        max_tokens: defaults.max_tokens,
        concurrency: defaults.concurrency,
        ..felin_core::llm::LlmConfig::default()
    };
    if let Some(e) = db.get_setting("llm_endpoint").map_err(|e| e.to_string())?.filter(|s| !s.is_empty()) {
        cfg.endpoint = e;
    }
    if let Some(m) = db.get_setting("llm_model").map_err(|e| e.to_string())?.filter(|s| !s.is_empty()) {
        cfg.model = m;
    }
    if let Some(k) = db.get_setting("llm_api_key").map_err(|e| e.to_string())? {
        cfg.api_key = k;
    }
    Ok(cfg)
}

#[tauri::command]
pub fn get_llm_config(state: State<'_, AppState>) -> Result<LlmConfigView, String> {
    with_project(&state, |p| {
        Ok(LlmConfigView {
            endpoint: p
                .db
                .get_setting("llm_endpoint")?
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| felin_core::llm::DEFAULT_ENDPOINT.to_string()),
            model: p
                .db
                .get_setting("llm_model")?
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| felin_core::llm::DEFAULT_MODEL.to_string()),
            has_key: p.db.get_setting("llm_api_key")?.is_some_and(|k| !k.is_empty()),
        })
    })
}

#[tauri::command]
pub fn set_llm_config(
    state: State<'_, AppState>,
    endpoint: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
) -> Result<(), String> {
    with_project(&state, |p| {
        if let Some(e) = endpoint {
            p.db.set_setting("llm_endpoint", &e)?;
        }
        if let Some(m) = model {
            p.db.set_setting("llm_model", &m)?;
        }
        if let Some(k) = api_key {
            p.db.set_setting("llm_api_key", &k)?;
        }
        Ok(())
    })
}

#[tauri::command]
pub fn csv_headers(state: State<'_, AppState>, path: String) -> Result<Vec<String>, String> {
    let data = read_regular_capped(Path::new(&path), state.config.import.max_file_bytes)?;
    names::csv::headers(&data).map_err(|e| e.to_string())
}

/// Preview the first `limit` parsed rows of a glossary CSV under `mapping` —
/// the read-only "按当前列映射的前几行解析结果" the import card shows so the
/// user can confirm their column selection (and see that unmapped columns are
/// dropped) before committing to the import.
#[tauri::command]
pub fn csv_preview(
    state: State<'_, AppState>,
    path: String,
    mapping: CsvMapping,
    limit: Option<usize>,
) -> Result<Vec<names::NameRow>, String> {
    let data = read_regular_capped(Path::new(&path), state.config.import.max_file_bytes)?;
    let rows = names::csv::parse(&data, &mapping.into_core()).map_err(|e| e.to_string())?;
    let limit = limit.unwrap_or(5);
    Ok(rows.into_iter().take(limit).collect())
}

/// Import a glossary CSV. `target` = `"project"` (project small glossary) or
/// `"global"` (shared big glossary). Project-target rows are ALSO upserted into
/// the global pool (accumulating the shared glossary) with a source tag naming
/// the project.
#[tauri::command]
pub fn import_glossary_csv(
    state: State<'_, AppState>,
    path: String,
    mapping: CsvMapping,
    target: String,
) -> Result<usize, String> {
    let data = read_regular_capped(Path::new(&path), state.config.import.max_file_bytes)?;
    let rows = names::csv::parse(&data, &mapping.into_core()).map_err(|e| e.to_string())?;
    let to_project = match target.as_str() {
        "project" => true,
        "global" => false,
        other => return Err(format!("unknown glossary target: {other}")),
    };
    let guard = state.project_guard();
    let proj = guard.as_ref();
    if to_project && proj.is_none() {
        return Err("no project is open".into());
    }
    let mut n = 0;
    for row in &rows {
        let source = if to_project {
            format!("project:{}", proj.unwrap().slug)
        } else {
            "imported".to_string()
        };
        let id = state
            .global
            .upsert_name_full(
                &row.japanese,
                Some(&row.chinese),
                row.english.as_deref(),
                row.category.as_deref(),
                row.notes.as_deref(),
                &source,
                NameStatus::Imported,
            )
            .map_err(|e| e.to_string())?;
        if to_project {
            // Carry over any existing global tags plus the source-project tag.
            let mut tags: Vec<String> = state
                .global
                .get_name(id)
                .map_err(|e| e.to_string())?
                .map(|g| g.tags)
                .unwrap_or_default();
            if !tags.contains(&source) {
                tags.push(source.clone());
            }
            state.global.set_name_tags(id, &tags).map_err(|e| e.to_string())?;
            let p = proj.unwrap();
            p.db.insert_glossary_entry(
                Some(id),
                &row.japanese,
                Some(&row.chinese),
                row.english.as_deref(),
                row.category.as_deref(),
                &tags,
                row.notes.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        }
        n += 1;
    }
    Ok(n)
}


// PLACEHOLDER_NAMES_REVIEW

/// Run the LLM name-extraction pass over the project's chapters, inserting new
/// candidates. Returns the number of new candidates added.
#[tauri::command]
pub async fn run_name_extraction(state: State<'_, AppState>) -> Result<usize, String> {
    // Gather the client + chapter texts under the lock, then release it before
    // awaiting the network calls. The extraction system message comes from
    // felin.toml `[prompt].extract_system` (empty → no system message, just the
    // chapter text — the config file is the single source of prompt text).
    let (client, chapters, extract_system) = {
        let guard = state.project_guard();
        let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
        let client = LlmClient::with_limiter(
            load_llm_config(&proj.db, &state.config.llm)?,
            Arc::clone(&state.llm_limiter),
        )
        .map_err(|e| e.to_string())?;
        let extract_system = state.prompt_config().extract_system.clone();
        let mut chapters = Vec::new();
        for ch in proj.db.list_chapters().map_err(|e| e.to_string())? {
            let text = proj
                .db
                .list_paragraphs(ch.id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|p| p.text)
                .collect::<Vec<_>>()
                .join("\n\n");
            chapters.push(text);
        }
        (client, chapters, extract_system)
    };

    let candidates = names::extract_names(&client, &chapters, &extract_system).await;

    let guard = state.project_guard();
    let proj = guard.as_ref().ok_or_else(|| "project was closed during extraction".to_string())?;
    let mut added = 0;
    for c in &candidates {
        let jp = names::normalize(c.japanese.trim());
        if jp.is_empty() {
            continue;
        }
        let zh = c.guess_chinese.trim();
        let note = c.context.trim();
        let cat = c.proposed_category();
        let inserted = proj
            .db
            .insert_extracted(
                &jp,
                (!zh.is_empty()).then_some(zh),
                (!cat.is_empty()).then_some(cat.as_str()),
                (!note.is_empty()).then_some(note),
            )
            .map_err(|e| e.to_string())?;
        if inserted.is_some() {
            added += 1;
        }
    }
    Ok(added)
}

#[tauri::command]
pub fn list_extracted(
    state: State<'_, AppState>,
    status: Option<String>,
) -> Result<Vec<ExtractedName>, String> {
    let status = match status.as_deref() {
        Some(s) => Some(s.parse::<ExtractedNameStatus>().map_err(|e| e.to_string())?),
        None => None,
    };
    with_project(&state, |p| p.db.list_extracted_names(status))
}

#[tauri::command]
pub fn update_extracted(state: State<'_, AppState>, id: i64, chinese: String) -> Result<(), String> {
    with_project(&state, |p| p.db.update_extracted_chinese(id, &chinese))
}

/// Edit a candidate's japanese form (OCR may misread, so it's user-editable
/// like the Chinese). Rejects renames that collide with another candidate.
#[tauri::command]
pub fn update_extracted_japanese(
    state: State<'_, AppState>,
    id: i64,
    japanese: String,
) -> Result<(), String> {
    with_project(&state, |p| p.db.update_extracted_japanese(id, &japanese))
}

/// Replace one candidate's category tags (JSON array). User-edited tags are
/// honored verbatim; nothing downstream re-derives them.
#[tauri::command]
pub fn update_extracted_tags(
    state: State<'_, AppState>,
    id: i64,
    tags: Vec<String>,
) -> Result<(), String> {
    with_project(&state, |p| p.db.set_extracted_tags(id, &tags))
}

/// Run the LLM classification pass over the given candidate ids, writing each
/// returned category into the candidate's tags (first tag wins; empty
/// categories skipped). Candidates are fetched under the project lock, then the
/// network call runs without it. `state.prompt.extract_tags_system` is the
/// classification prompt from `felin.toml [prompt]`; when it is empty the
/// command refuses with a hint (找不到即报错 — no silent no-op). Returns how many
/// candidates got a category.
#[tauri::command]
pub async fn auto_tag_extracted(state: State<'_, AppState>, ids: Vec<i64>) -> Result<usize, String> {
    // Dedup, then fetch the japanese forms + LLM client under the lock; the
    // network call runs after the lock is released.
    let (client, forms) = {
        let guard = state.project_guard();
        let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
        let client = LlmClient::with_limiter(
            load_llm_config(&proj.db, &state.config.llm)?,
            Arc::clone(&state.llm_limiter),
        )
        .map_err(|e| e.to_string())?;
        let mut seen = std::collections::HashSet::new();
        let mut forms: Vec<String> = Vec::new();
        for id in ids {
            if !seen.insert(id) {
                continue;
            }
            if let Some(c) = proj.db.get_extracted(id).map_err(|e| e.to_string())? {
                forms.push(c.japanese);
            }
        }
        (client, forms)
    };
    let classify_system = state.prompt_config().extract_tags_system.clone();
    let suggestions = names::classify_names(&client, &forms, &classify_system).await;
    if suggestions.is_empty() {
        return Ok(0);
    }
    let mut applied = 0;
    let guard = state.project_guard();
    let proj = guard.as_ref().ok_or_else(|| "project was closed during auto-tag".to_string())?;
    // The candidates the classification pass may have matched, indexed by
    // normalized japanese form (classification returns normalized forms).
    let by_form: std::collections::HashMap<String, ExtractedName> = proj
        .db
        .list_extracted_names(None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|c| (c.japanese.clone(), c))
        .collect();
    for s in &suggestions {
        let cat = s.category.trim().to_string();
        if cat.is_empty() {
            continue;
        }
        let Some(c) = by_form.get(&s.japanese) else { continue };
        // First tag wins (existing user/LLM tags are preserved).
        if !c.tags.is_empty() {
            continue;
        }
        proj.db.set_extracted_tags(c.id, &[cat]).map_err(|e| e.to_string())?;
        applied += 1;
    }
    Ok(applied)
}

/// Batch-set the same category tags on many candidates — the checkbox-driven
/// 「批量标记」 action on the extraction card (also used for 全选 + 批量确认,
/// where the whole selection should share the chosen tag). Returns how many
/// candidates were updated.
#[tauri::command]
pub fn apply_extracted_tags(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    tags: Vec<String>,
) -> Result<usize, String> {
    let mut n = 0;
    with_project(&state, |p| {
        for id in ids {
            p.db.set_extracted_tags(id, &tags)?;
            n += 1;
        }
        Ok(n)
    })
}

#[tauri::command]
pub fn reject_extracted(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    with_project(&state, |p| p.db.set_extracted_status(id, ExtractedNameStatus::Rejected, None))
}

/// Batch-reject extracted candidates (mark `Rejected`). Returns how many were
/// processed.
#[tauri::command]
pub fn reject_extracted_batch(state: State<'_, AppState>, ids: Vec<i64>) -> Result<usize, String> {
    with_project(&state, |p| {
        let mut n = 0usize;
        for id in ids {
            p.db.set_extracted_status(id, ExtractedNameStatus::Rejected, None)?;
            n += 1;
        }
        Ok(n)
    })
}

/// Resolve the glossary target string shared by the confirm commands.
fn parse_target(target: &str) -> Result<bool, String> {
    match target {
        "project" => Ok(true),
        "global" => Ok(false),
        other => Err(format!("unknown glossary target: {other}")),
    }
}

/// Confirm one candidate into a glossary — the shared body of
/// [`confirm_extracted`] / [`confirm_extracted_batch`]. `to_project` = the
/// candidate lands in the project's self-contained small glossary (it is always
/// upserted into the global pool too, accumulating the shared glossary with a
/// source tag naming this project).
fn confirm_extracted_one(
    state: &AppState,
    proj: &OpenProject,
    id: i64,
    to_project: bool,
) -> Result<(), String> {
    let cand = proj
        .db
        .get_extracted(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "candidate not found".to_string())?;
    // The candidate's own category tags (LLM-proposed or user-edited) are the
    // canonical value. For the global pool, only fill the category when the
    // existing entry has none — an established global category (from another
    // project) must not be clobbered by one candidate's tag; the tags array
    // keeps both. The small glossary is project-local and self-contained, so it
    // takes the candidate's category directly.
    let cand_tags = cand.tags;
    let cand_category = cand_tags.first().cloned();
    let source = format!("project:{}", proj.slug);
    let name_id = state
        .global
        .upsert_name_full(
            &cand.japanese,
            cand.candidate_chinese.as_deref(),
            None,
            None, // category decided below (preserve established global category)
            None,
            &source,
            NameStatus::Draft,
        )
        .map_err(|e| e.to_string())?;
    // Carry the existing global tags forward, stamp the source project, and add
    // any candidate categories not already present.
    let mut tags: Vec<String> = state
        .global
        .get_name(name_id)
        .map_err(|e| e.to_string())?
        .map(|g| g.tags)
        .unwrap_or_default();
    if !tags.contains(&source) {
        tags.push(source.clone());
    }
    for t in &cand_tags {
        if !tags.contains(t) {
            tags.push(t.clone());
        }
    }
    state.global.set_name_tags(name_id, &tags).map_err(|e| e.to_string())?;
    // The global category is filled only when the entry has none — an
    // established category from another project is preserved (the tags array
    // already carries both). The small glossary is project-local, so it takes
    // the candidate's category directly.
    let existing_category = state
        .global
        .get_name(name_id)
        .map_err(|e| e.to_string())?
        .and_then(|g| g.category)
        .filter(|c| !c.trim().is_empty());
    if let (Some(cat), None) = (&cand_category, existing_category) {
        state.global.set_name_category(name_id, cat).map_err(|e| e.to_string())?;
    }
    if to_project {
        proj.db
            .insert_glossary_entry(
                Some(name_id),
                &cand.japanese,
                cand.candidate_chinese.as_deref(),
                None,
                cand_category.as_deref(),
                &tags,
                None,
            )
            .map_err(|e| e.to_string())?;
    }
    proj.db
        .set_extracted_status(id, ExtractedNameStatus::Confirmed, Some(name_id))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn confirm_extracted(
    state: State<'_, AppState>,
    id: i64,
    target: String,
) -> Result<(), String> {
    let to_project = parse_target(&target)?;
    let guard = state.project_guard();
    let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
    confirm_extracted_one(&state, proj, id, to_project)
}

/// Batch-confirm candidates into a glossary (`target` = `"project"` / `"global"`),
/// running each through the same path as [`confirm_extracted`]. Executed
/// per-candidate: on a failure the run stops and the error reports how many
/// succeeded (already-confirmed candidates stay confirmed — no rollback).
/// Returns the number of candidates confirmed.
#[tauri::command]
pub fn confirm_extracted_batch(
    state: State<'_, AppState>,
    ids: Vec<i64>,
    target: String,
) -> Result<usize, String> {
    let to_project = parse_target(&target)?;
    let guard = state.project_guard();
    let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
    let mut done = 0usize;
    for id in ids {
        if let Err(e) = confirm_extracted_one(&state, proj, id, to_project) {
            return Err(format!("第 {} 条确认失败（已确认 {done} 条）：{e}", done + 1));
        }
        done += 1;
    }
    Ok(done)
}




