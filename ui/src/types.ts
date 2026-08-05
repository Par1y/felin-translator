// Mirrors of the Rust command payloads (src-tauri/src/commands.rs).

export interface AppInfo {
  version: string;
  data_dir: string;
  sidecar: string;
  sidecar_present: boolean;
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
  archive: string;
  sha256: string;
  bytes: number;
  files: number;
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
}

export interface ExtractedName {
  id: number;
  japanese: string;
  matched_name_id: number | null;
  candidate_chinese: string | null;
  status: string;
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
  english?: number;
  category?: number;
  notes?: number;
  aliases?: number;
  has_header: boolean;
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
