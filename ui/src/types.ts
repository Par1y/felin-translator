// Mirrors of the Rust command payloads (src-tauri/src/commands.rs).

export interface AppInfo {
  version: string;
  data_dir: string;
  sidecar: string;
  sidecar_present: boolean;
  ocr_config_path: string;
  ocr_config_present: boolean;
  glossary_names: number;
}

export interface ProjectSummary {
  slug: string;
  name: string;
  created_at: string;
}

export interface Chapter {
  id: number;
  title: string;
  ord: number;
  status: string;
}

export interface Paragraph {
  id: string;
  chapter_id: number;
  ord: number;
  text: string;
  page_num: number | null;
  page_score: number | null;
  ocr_status: string;
  ocr_meta: unknown | null;
  source_file: string | null;
}

export interface ImportResult {
  task_id: string;
  outcome: string;
  pages_ok: number;
  pages_failed: number;
  failed_pages: number[];
  paragraphs: number;
  chapter_id: number;
}

export interface TxtImportResult {
  chapter_id: number;
  paragraphs: number;
}

export interface ExportResult {
  task_id: string;
  archive: string;
  sha256: string;
  bytes: number;
  files: number;
}

// Mirrors felin_core::archive::ArchiveProgress (tagged by "event").
export type ArchiveProgressEvent =
  | { event: "start"; total_files: number }
  | { event: "progress"; done: number; total_files: number };

export interface ExportProgressPayload {
  task_id: string;
  event: ArchiveProgressEvent;
}

export interface Tu {
  id: number;
  chapter_id: number;
  paragraph_ids: string[];
  ord: number;
  budget: number | null;
  status: string;
}

export interface SegmentResult {
  chapters: number;
  tus: number;
}

export interface GlossaryName {
  id: number;
  japanese: string;
  chinese: string | null;
  english: string | null;
  category: string | null;
  notes: string | null;
  source: string | null;
  status: string;
  tags: string[];
  enabled: boolean;
}

// Mirrors felin_core::types::GlossaryEntry (project small glossary).
export interface GlossaryEntry {
  id: number;
  name_global_id: number | null;
  japanese: string;
  chinese: string | null;
  english: string | null;
  category: string | null;
  tags: string[];
  enabled: boolean;
  notes: string | null;
  created_at: string;
  updated_at: string;
}

// Input fields for add/update glossary commands (minus the row id / provenance).
export interface GlossaryEntryInput {
  japanese: string;
  chinese: string | null;
  english: string | null;
  category: string | null;
  tags: string[];
  notes: string | null;
}

export interface AddGlossaryEntryInput extends GlossaryEntryInput {
  name_global_id: number | null;
}

// Mirrors felin_core::types::OcrSettings (per-project batch options).
export interface OcrSettings {
  batch_workers: number;
  batch_recursive: boolean;
}

// Mirrors felin_core::ocr::config (the app-editable slice of ocr-router's
// config.yaml, edited in place).
export interface OcrProviderConfig {
  name: string;
  enabled: boolean;
  endpoint: string;
  model: string;
  api_key: string;
}

export interface OcrEvaluatorConfig {
  enabled: boolean;
  endpoint: string;
  model: string;
  api_key: string;
}

export interface OcrConfig {
  providers: OcrProviderConfig[];
  order: string[];
  evaluator: OcrEvaluatorConfig;
}

// Mirrors felin_core::types::FileSelection (image-dir scan preview).
export interface FileSelection {
  total: number;
  matched: number;
  names: string[];
  bytes: number;
}

// Mirrors felin_core::types::TranslationExport (deterministic 译文导出).
export interface TranslationExport {
  txt_path: string;
  csv_path: string;
  tus: number;
}

// Mirrors felin_core::ocr::select::ImagePreset (serde snake_case).
export type ImagePreset = "all" | "png" | "jpg" | "numbered" | "numbered_prefix";

// Mirrors felin_core::ocr::select::ImageMatchRule.
export interface ImageMatchRule {
  preset: ImagePreset;
  custom_glob: string | null;
  custom_regex: string | null;
  /** Inclusive 1-based page range in natural reading order; null = no cut. */
  range: [number, number] | null;
}

export const DEFAULT_IMAGE_RULE: ImageMatchRule = {
  preset: "all",
  custom_glob: null,
  custom_regex: null,
  range: null,
};

// Mirrors felin_core::types::ExtractedName (name-extraction candidates).
export interface ExtractedName {
  id: number;
  japanese: string;
  matched_name_id: number | null;
  candidate_chinese: string | null;
  status: string;
  /** Category tags proposed by the LLM / edited by the user (人名、地名…). */
  tags: string[];
  notes: string | null;
}

export interface LlmConfigView {
  endpoint: string;
  model: string;
  has_key: boolean;
}

export interface CsvMapping {
  japanese: number;
  chinese: number;
  english?: number | null;
  category?: number | null;
  notes?: number | null;
  has_header: boolean;
}

/// One parsed glossary row, as returned by `csv_preview` (unmapped optional
/// fields come back `null`).
export interface CsvPreviewRow {
  japanese: string;
  chinese: string;
  english: string | null;
  category: string | null;
  notes: string | null;
}

// Matches felin_core::ocr::contract::ProgressEvent (tagged by "event").
export type ProgressEvent =
  | { event: "start"; source: string; pages_total: number }
  | {
      event: "page";
      page: number;
      status: "ok" | "failed";
      score: number | null;
      error: string | null;
      done: number;
      total: number;
    }
  | { event: "done"; pages_ok: number; pages_failed: number; manifest: string | null };

export interface ProgressPayload {
  task_id: string;
  event: ProgressEvent;
}

export interface ErrorPayload {
  task_id: string;
  message: string;
}

// ----- translation pipeline (plan step 8) ----------------------------------

// Mirrors felin_core::pipeline::PipelineEvent (tagged by "event").
export type PipelineEvent =
  | { event: "started"; total_tus: number }
  | { event: "tu_start"; tu_id: number }
  | { event: "tu_done"; tu_id: number; memory_hit: boolean }
  | { event: "tu_failed"; tu_id: number; error: string }
  | { event: "stopped" }
  | { event: "finished"; total_tus: number };

export interface TranslationProgressPayload {
  task_id: string;
  event: PipelineEvent;
}

export interface TranslationDonePayload {
  task_id: string;
}

// Mirrors felin_core::types::TranslationSettings (per-project GUI options).
export interface TranslationSettings {
  workers: number;
  window: number;
  memory_dedup: boolean;
  stop_aborts_inflight: boolean;
}

// Mirrors felin_core::config::PromptConfig (felin.toml [prompt] templates).
// Empty field = that message section isn't sent.
export interface PromptConfig {
  extract_system: string;
  /** Name-classification message for the auto-tag pass (empty → auto-tag refuses). */
  extract_tags_system: string;
  translation_system: string;
  translation_user: string;
}

export interface StatusCount {
  status: string;
  count: number;
}

export interface TranslationStatusView {
  running: boolean;
  task_id: string | null;
  workers: number;
  window: number;
  active_chapters: number[];
  counts: StatusCount[];
}

// Mirrors felin_core::types::MatchedName — a proper noun a TU's source hit.
export interface MatchedName {
  /** The entry's canonical japanese form (as it matched). */
  japanese: string;
  /** The entry's Chinese rendering (null when unset). */
  chinese: string | null;
}

// Mirrors felin_core::types::TuWithTranslation.
export interface TuWithTranslation {
  id: number;
  ord: number;
  budget: number | null;
  status: string;
  translation_status: string | null;
  /** Effective source: the user's source_override if set, else the paragraphs. */
  source: string;
  /** Enabled small-glossary entries this source hit (what prompt injection applied). */
  matched_names: MatchedName[];
  final_text: string | null;
  llm_text: string | null;
  instruction: string | null;
  error: string | null;
  source_hash: string | null;
}
