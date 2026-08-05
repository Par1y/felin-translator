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
use felin_core::llm::LlmClient;
use felin_core::names;
use felin_core::ocr::sidecar::run_extract;
use felin_core::ocr::{
    ingest_from_manifest, read_manifest, ExtractArgs, ExtractOutcome, ProgressEvent,
};
use felin_core::storage::{DbTuning, ProjectDb, ProjectLock};
use felin_core::types::{Chapter, ExtractedName, ExtractedNameStatus, GlossaryName, NameStatus, Paragraph, Tu};
use felin_core::util::now_iso8601;
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::watch;

// PLACEHOLDER_TYPES

#[derive(Serialize, Clone)]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    pub sidecar: String,
    pub sidecar_present: bool,
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
    pub archive: String,
    pub sha256: String,
    pub bytes: u64,
    pub files: usize,
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
        sidecar: state.sidecar.display().to_string(),
        sidecar_present: state.sidecar.exists(),
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
    db.set_setting("project_name", &name).map_err(|e| e.to_string())?;
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
    if !state.sidecar.exists() {
        return Err(format!("OCR sidecar not found at {}", state.sidecar.display()));
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

    let args = ExtractArgs {
        sidecar: state.sidecar.clone(),
        input: PathBuf::from(&input),
        config: None,
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

// PLACEHOLDER_ARCHIVE

/// Export the currently-open project to a single compressed archive at
/// `dest_path` (a location the user picks — e.g. next to their source files),
/// with a SHA-256 digest for integrity. Lets a project be moved/backed up even
/// though its data normally lives next to the software.
#[tauri::command]
pub fn export_project(state: State<'_, AppState>, dest_path: String) -> Result<ExportResult, String> {
    let (root, slug) = {
        let guard = state.project_guard();
        let p = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
        (p.root.clone(), p.slug.clone())
    };
    let dest = PathBuf::from(&dest_path);
    let out = archive::export_project(&root, &slug, &dest).map_err(|e| e.to_string())?;
    Ok(ExportResult {
        archive: dest.display().to_string(),
        sha256: out.sha256,
        bytes: out.bytes,
        files: out.files,
    })
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
    pub aliases: Option<usize>,
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
            aliases: self.aliases,
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

#[tauri::command]
pub fn import_glossary_csv(
    state: State<'_, AppState>,
    path: String,
    mapping: CsvMapping,
) -> Result<usize, String> {
    let data = read_regular_capped(Path::new(&path), state.config.import.max_file_bytes)?;
    let rows = names::csv::parse(&data, &mapping.into_core()).map_err(|e| e.to_string())?;
    let mut n = 0;
    for row in &rows {
        let id = state
            .global
            .upsert_name_full(
                &row.japanese,
                Some(&row.chinese),
                row.english.as_deref(),
                row.category.as_deref(),
                row.notes.as_deref(),
                "imported",
                NameStatus::Imported,
            )
            .map_err(|e| e.to_string())?;
        for a in &row.aliases {
            state.global.add_alias(id, a).map_err(|e| e.to_string())?;
        }
        n += 1;
    }
    Ok(n)
}

#[tauri::command]
pub fn list_glossary(state: State<'_, AppState>, limit: Option<i64>) -> Result<Vec<GlossaryName>, String> {
    state.global.list_names(limit.unwrap_or(500)).map_err(|e| e.to_string())
}

// PLACEHOLDER_NAMES_REVIEW

/// Run the LLM name-extraction pass over the project's chapters, inserting new
/// candidates. Returns the number of new candidates added.
#[tauri::command]
pub async fn run_name_extraction(state: State<'_, AppState>) -> Result<usize, String> {
    // Gather the client + chapter texts under the lock, then release it before
    // awaiting the network calls.
    let (client, chapters) = {
        let guard = state.project_guard();
        let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
        let client = LlmClient::new(load_llm_config(&proj.db, &state.config.llm)?).map_err(|e| e.to_string())?;
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
        (client, chapters)
    };

    let candidates = names::extract_names(&client, &chapters).await;

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
        let inserted = proj
            .db
            .insert_extracted(&jp, (!zh.is_empty()).then_some(zh), (!note.is_empty()).then_some(note))
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

#[tauri::command]
pub fn reject_extracted(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    with_project(&state, |p| p.db.set_extracted_status(id, ExtractedNameStatus::Rejected, None))
}

/// Confirm a candidate: upsert it into the global glossary (source = this
/// project, status draft) and mark the candidate confirmed + linked.
#[tauri::command]
pub fn confirm_extracted(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let guard = state.project_guard();
    let proj = guard.as_ref().ok_or_else(|| "no project is open".to_string())?;
    let cand = proj
        .db
        .get_extracted(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "candidate not found".to_string())?;
    let source = format!("project:{}", proj.slug);
    let name_id = state
        .global
        .upsert_name_full(
            &cand.japanese,
            cand.candidate_chinese.as_deref(),
            None,
            None,
            None,
            &source,
            NameStatus::Draft,
        )
        .map_err(|e| e.to_string())?;
    proj.db
        .set_extracted_status(id, ExtractedNameStatus::Confirmed, Some(name_id))
        .map_err(|e| e.to_string())?;
    Ok(())
}




