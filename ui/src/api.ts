import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AddGlossaryEntryInput,
  AppInfo,
  Chapter,
  CsvMapping,
  CsvPreviewRow,
  ErrorPayload,
  ExportResult,
  ExtractedName,
  FileSelection,
  GlossaryEntry,
  GlossaryEntryInput,
  GlossaryName,
  ImageMatchRule,
  ImportResult,
  LlmConfigView,
  OcrConfig,
  OcrSettings,
  Paragraph,
  ProgressPayload,
  ProjectSummary,
  PromptConfig,
  SegmentResult,
  TranslationDonePayload,
  TranslationExport,
  TranslationProgressPayload,
  TranslationSettings,
  TranslationStatusView,
  Tu,
  TuWithTranslation,
  TxtImportResult,
} from "./types";

/// Typed wrappers over the Tauri command surface. Tauri v2 maps camelCase JS
/// keys to snake_case Rust parameters automatically.
export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  createProject: (name: string) => invoke<ProjectSummary>("create_project", { name }),
  renameProject: (slug: string, name: string) =>
    invoke<ProjectSummary>("rename_project", { slug, name }),
  deleteProject: (slug: string) => invoke<void>("delete_project", { slug }),
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
  scanImageDir: (dir: string, rule: ImageMatchRule) =>
    invoke<FileSelection>("scan_image_dir", { dir, rule }),
  importImagesBatch: (dir: string, rule: ImageMatchRule) =>
    invoke<string>("import_images_batch", { dir, rule }),
  exportProject: (destPath: string) => invoke<ExportResult>("export_project", { destPath }),
  importProject: (archivePath: string) => invoke<ProjectSummary>("import_project", { archivePath }),

  getLlmConfig: () => invoke<LlmConfigView>("get_llm_config"),
  setLlmConfig: (endpoint?: string, model?: string, apiKey?: string) =>
    invoke<void>("set_llm_config", { endpoint: endpoint ?? null, model: model ?? null, apiKey: apiKey ?? null }),
  testLlmConnection: () => invoke<void>("test_llm_connection"),
  csvHeaders: (path: string) => invoke<string[]>("csv_headers", { path }),
  csvPreview: (path: string, mapping: CsvMapping, limit?: number) =>
    invoke<CsvPreviewRow[]>("csv_preview", { path, mapping, limit: limit ?? null }),
  importGlossaryCsv: (path: string, mapping: CsvMapping, target: "project" | "global") =>
    invoke<number>("import_glossary_csv", { path, mapping, target }),
  listGlossary: (q?: string, limit?: number) =>
    invoke<GlossaryName[]>("list_glossary", { q: q ?? null, limit: limit ?? null }),
  setGlobalNameTags: (id: number, tags: string[]) =>
    invoke<void>("set_global_name_tags", { id, tags }),
  setGlobalNameEnabled: (id: number, enabled: boolean) =>
    invoke<void>("set_global_name_enabled", { id, enabled }),
  deleteGlobalNames: (ids: number[]) => invoke<number>("delete_global_names", { ids }),
  runNameExtraction: () => invoke<number>("run_name_extraction"),
  listExtracted: (status?: string) => invoke<ExtractedName[]>("list_extracted", { status: status ?? null }),
  updateExtracted: (id: number, chinese: string) => invoke<void>("update_extracted", { id, chinese }),
  updateExtractedTags: (id: number, tags: string[]) =>
    invoke<void>("update_extracted_tags", { id, tags }),
  autoTagExtracted: (ids: number[]) => invoke<number>("auto_tag_extracted", { ids }),
  applyExtractedTags: (ids: number[], tags: string[]) =>
    invoke<number>("apply_extracted_tags", { ids, tags }),
  rejectExtracted: (id: number) => invoke<void>("reject_extracted", { id }),
  rejectExtractedBatch: (ids: number[]) => invoke<number>("reject_extracted_batch", { ids }),
  confirmExtracted: (id: number, target: "project" | "global") =>
    invoke<void>("confirm_extracted", { id, target }),
  confirmExtractedBatch: (ids: number[], target: "project" | "global") =>
    invoke<number>("confirm_extracted_batch", { ids, target }),

  startTranslation: () => invoke<string>("start_translation"),
  stopTranslation: () => invoke<void>("stop_translation"),
  translationStatus: () => invoke<TranslationStatusView>("translation_status"),
  retryTranslation: (scope: "tu" | "chapter" | "all", ids: number[]) =>
    invoke<number>("retry_translation", { scope, ids }),
  approveTu: (tuId: number) => invoke<boolean>("approve_tu", { tuId }),
  setTuInstruction: (tuId: number, instruction: string) =>
    invoke<void>("set_tu_instruction", { tuId, instruction }),
  retranslateTu: (tuId: number, instruction: string) =>
    invoke<boolean>("retranslate_tu", { tuId, instruction }),
  retranslateTus: (ids: number[], instruction?: string) =>
    invoke<number>("retranslate_tus", { ids, instruction: instruction ?? null }),
  getTranslationSettings: () => invoke<TranslationSettings>("get_translation_settings"),
  setTranslationSettings: (s: TranslationSettings) =>
    invoke<void>("set_translation_settings", { settings: s }),
  getGuidelines: () => invoke<string>("get_guidelines"),
  setGuidelines: (text: string) => invoke<void>("set_guidelines", { text }),
  getPromptConfig: () => invoke<PromptConfig>("get_prompt_config"),
  setPromptConfig: (config: PromptConfig) => invoke<void>("set_prompt_config", { config }),
  listTusWithTranslations: (chapterId: number) =>
    invoke<TuWithTranslation[]>("list_tus_with_translations", { chapterId }),
  setTuSource: (tuId: number, source: string) => invoke<void>("set_tu_source", { tuId, source }),
  setTranslationText: (tuId: number, text: string) =>
    invoke<boolean>("set_translation_text", { tuId, text }),
  deleteTus: (ids: number[]) => invoke<number>("delete_tus", { ids }),

  getOcrSettings: () => invoke<OcrSettings>("get_ocr_settings"),
  setOcrSettings: (s: OcrSettings) => invoke<void>("set_ocr_settings", { settings: s }),
  getOcrConfig: () => invoke<OcrConfig>("get_ocr_config"),
  setOcrConfig: (c: OcrConfig) => invoke<void>("set_ocr_config", { config: c }),
  exportTranslations: (destDir: string) =>
    invoke<TranslationExport>("export_translations", { destDir }),

  listGlossaryEntries: (q?: string) =>
    invoke<GlossaryEntry[]>("list_glossary_entries", { q: q ?? null }),
  addGlossaryEntry: (e: AddGlossaryEntryInput) => invoke<number>("add_glossary_entry", { ...e }),
  updateGlossaryEntry: (id: number, e: GlossaryEntryInput) =>
    invoke<void>("update_glossary_entry", { id, ...e }),
  setEntryEnabled: (id: number, enabled: boolean) =>
    invoke<void>("set_entry_enabled", { id, enabled }),
  setEntryTags: (id: number, tags: string[]) => invoke<void>("set_entry_tags", { id, tags }),
  deleteGlossaryEntry: (id: number) => invoke<void>("delete_glossary_entry", { id }),
  deleteGlossaryEntries: (ids: number[]) => invoke<number>("delete_glossary_entries", { ids }),
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

export function onTranslationProgress(
  cb: (p: TranslationProgressPayload) => void,
): Promise<UnlistenFn> {
  return listen<TranslationProgressPayload>("translation://progress", (e) => cb(e.payload));
}

export function onTranslationDone(cb: (r: TranslationDonePayload) => void): Promise<UnlistenFn> {
  return listen<TranslationDonePayload>("translation://done", (e) => cb(e.payload));
}

export function onTranslationError(cb: (e: ErrorPayload) => void): Promise<UnlistenFn> {
  return listen<ErrorPayload>("translation://error", (e) => cb(e.payload));
}
