import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppInfo,
  Chapter,
  CsvMapping,
  ErrorPayload,
  ExportResult,
  ExtractedName,
  GlossaryName,
  ImportResult,
  LlmConfigView,
  Paragraph,
  ProgressPayload,
  ProjectSummary,
  SegmentResult,
  Tu,
  TxtImportResult,
} from "./types";

/// Typed wrappers over the Tauri command surface. Tauri v2 maps camelCase JS
/// keys to snake_case Rust parameters automatically.
export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  createProject: (name: string) => invoke<ProjectSummary>("create_project", { name }),
  openProject: (slug: string) => invoke<ProjectSummary>("open_project", { slug }),
  closeProject: () => invoke<void>("close_project"),
  currentProject: () => invoke<ProjectSummary | null>("current_project"),
  listProjects: () => invoke<ProjectSummary[]>("list_projects"),
  listChapters: () => invoke<Chapter[]>("list_chapters"),
  listParagraphs: (chapterId: number) => invoke<Paragraph[]>("list_paragraphs", { chapterId }),
  listTus: (chapterId: number) => invoke<Tu[]>("list_tus", { chapterId }),
  segmentProject: (budget?: number) =>
    invoke<SegmentResult>("segment_project", { budget: budget ?? null }),
  importTxtFile: (path: string) => invoke<TxtImportResult>("import_txt_file", { path }),
  importOcr: (input: string, pages?: string) =>
    invoke<string>("import_ocr", { input, pages: pages ?? null }),
  cancelImport: (taskId: string) => invoke<void>("cancel_import", { taskId }),
  exportProject: (destPath: string) => invoke<ExportResult>("export_project", { destPath }),
  importProject: (archivePath: string) => invoke<ProjectSummary>("import_project", { archivePath }),

  getLlmConfig: () => invoke<LlmConfigView>("get_llm_config"),
  setLlmConfig: (endpoint?: string, model?: string, apiKey?: string) =>
    invoke<void>("set_llm_config", { endpoint: endpoint ?? null, model: model ?? null, apiKey: apiKey ?? null }),
  csvHeaders: (path: string) => invoke<string[]>("csv_headers", { path }),
  importGlossaryCsv: (path: string, mapping: CsvMapping) =>
    invoke<number>("import_glossary_csv", { path, mapping }),
  listGlossary: (limit?: number) => invoke<GlossaryName[]>("list_glossary", { limit: limit ?? null }),
  runNameExtraction: () => invoke<number>("run_name_extraction"),
  listExtracted: (status?: string) => invoke<ExtractedName[]>("list_extracted", { status: status ?? null }),
  updateExtracted: (id: number, chinese: string) => invoke<void>("update_extracted", { id, chinese }),
  rejectExtracted: (id: number) => invoke<void>("reject_extracted", { id }),
  confirmExtracted: (id: number) => invoke<void>("confirm_extracted", { id }),
};

export function onOcrProgress(cb: (p: ProgressPayload) => void): Promise<UnlistenFn> {
  return listen<ProgressPayload>("ocr://progress", (e) => cb(e.payload));
}

export function onOcrDone(cb: (r: ImportResult) => void): Promise<UnlistenFn> {
  return listen<ImportResult>("ocr://done", (e) => cb(e.payload));
}

export function onOcrError(cb: (e: ErrorPayload) => void): Promise<UnlistenFn> {
  return listen<ErrorPayload>("ocr://error", (e) => cb(e.payload));
}
