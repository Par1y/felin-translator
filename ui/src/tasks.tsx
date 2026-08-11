//! Global async-task store: the single owner of in-flight task state and Tauri
//! event subscriptions for OCR import, the translation pipeline, and project
//! export.
//!
//! Why a global store? Pages are swapped via conditional rendering in App.tsx,
//! which unmounts a page when you navigate away — taking its `useState`/`useRef`
//! and its `listen()` subscriptions with it. An OCR import started on the 导入
//! page would lose its progress display and cancel button the moment you switch
//! pages. Mounting the provider once at the app root (in main.tsx) keeps every
//! task observable and cancellable no matter which page is shown.
//!
//! Only task *lifecycle* state lives here. Form inputs (file paths, dirs,
//! rules, selections, chapter picks) stay local to their pages.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { App as AntdApp } from "antd";
import {
  api,
  onExportDone,
  onExportError,
  onExportProgress,
  onOcrDone,
  onOcrError,
  onOcrProgress,
  onTranslationDone,
  onTranslationError,
  onTranslationProgress,
} from "./api";
import type {
  ExportResult,
  ImportResult,
  TranslationStatusView,
} from "./types";

interface OcrTaskState {
  taskId: string | null;
  progress: { done: number; total: number } | null;
  /** Per-page OCR log lines; survives navigation. */
  log: string[];
  result: ImportResult | null;
}

interface TranslationTaskState {
  running: boolean;
  taskId: string | null;
  counts: { status: string; count: number }[];
  activeChapters: number[];
  /** Bumped on every pipeline event; pages watch it to refresh their TU list. */
  revision: number;
}

interface ExportTaskState {
  busy: boolean;
  taskId: string | null;
  progress: { done: number; total: number } | null;
  result: ExportResult | null;
}

interface TasksContextValue {
  ocr: OcrTaskState;
  translation: TranslationTaskState;
  /** "export" is a reserved word — the export slice is `exportTask`. */
  exportTask: ExportTaskState;

  ocrStart: (taskId: string) => void;
  ocrCancel: () => Promise<void>;
  ocrClear: () => void;

  translationStart: (taskId: string) => void;
  translationSync: (st: TranslationStatusView) => void;
  translationStop: () => Promise<void>;
  translationClear: () => void;

  exportStart: (taskId: string) => void;
  exportClear: () => void;

  clearAll: () => void;
}

const EMPTY_OCR: OcrTaskState = { taskId: null, progress: null, log: [], result: null };
const EMPTY_TRANSLATION: TranslationTaskState = {
  running: false,
  taskId: null,
  counts: [],
  activeChapters: [],
  revision: 0,
};
const EMPTY_EXPORT: ExportTaskState = { busy: false, taskId: null, progress: null, result: null };

const TasksContext = createContext<TasksContextValue | null>(null);

export function useTasks(): TasksContextValue {
  const ctx = useContext(TasksContext);
  if (!ctx) throw new Error("useTasks must be used within <TaskProvider>");
  return ctx;
}

export function TaskProvider({ children }: { children: ReactNode }) {
  const { message } = AntdApp.useApp();
  const [ocr, setOcr] = useState<OcrTaskState>(EMPTY_OCR);
  const [translation, setTranslation] = useState<TranslationTaskState>(EMPTY_TRANSLATION);
  const [exportTask, setExportTask] = useState<ExportTaskState>(EMPTY_EXPORT);

  // Refs mirror the stored taskId so event handlers never close over stale state.
  const ocrIdRef = useRef<string | null>(null);
  const transIdRef = useRef<string | null>(null);
  const exportIdRef = useRef<string | null>(null);
  useEffect(() => { ocrIdRef.current = ocr.taskId; }, [ocr.taskId]);
  useEffect(() => { transIdRef.current = translation.taskId; }, [translation.taskId]);
  useEffect(() => { exportIdRef.current = exportTask.taskId; }, [exportTask.taskId]);

  // ---- Mutators -------------------------------------------------------------

  const ocrStart = useCallback((taskId: string) =>
    setOcr({ taskId, progress: null, log: [], result: null }), []);

  const ocrCancel = useCallback(async () => {
    const id = ocrIdRef.current;
    if (!id) return;
    try {
      await api.cancelImport(id);
      message.info("已请求取消");
    } catch (e) {
      message.error(String(e));
    }
  }, [message]);

  const ocrClear = useCallback(() => setOcr(EMPTY_OCR), []);

  const translationStart = useCallback((taskId: string) =>
    setTranslation((s) => ({ ...s, taskId, running: true })), []);

  /// Push the backend's `translationStatus()` snapshot into the store
  /// (the recovery path for a run that started before the page mounted).
  const translationSync = useCallback((st: TranslationStatusView) =>
    setTranslation((s) => ({
      ...s,
      running: st.running,
      taskId: st.task_id,
      counts: st.counts,
      activeChapters: st.active_chapters,
    })), []);

  const translationStop = useCallback(async () => {
    try {
      await api.stopTranslation();
      message.info("已请求停止");
    } catch (e) {
      message.error(String(e));
    }
  }, [message]);

  const translationClear = useCallback(() => setTranslation(EMPTY_TRANSLATION), []);

  const exportStart = useCallback((taskId: string) =>
    setExportTask({ taskId, busy: true, progress: null, result: null }), []);

  const exportClear = useCallback(() => setExportTask(EMPTY_EXPORT), []);

  const clearAll = useCallback(() => {
    setOcr(EMPTY_OCR);
    setTranslation(EMPTY_TRANSLATION);
    setExportTask(EMPTY_EXPORT);
  }, []);

  // ---- Event subscriptions (once, at the root) ------------------------------

  useEffect(() => {
    const subs = [
      // ---- OCR import ----
      onOcrProgress((p) => {
        if (p.task_id !== ocrIdRef.current) return;
        const ev = p.event;
        if (ev.event === "start") {
          setOcr((s) => ({ ...s, log: [...s.log, `开始：共 ${ev.pages_total} 项`] }));
        } else if (ev.event === "page") {
          setOcr((s) => ({
            ...s,
            progress: { done: ev.done, total: ev.total },
            log: [...s.log, `第 ${ev.page} 项：${ev.status}${ev.error ? ` (${ev.error})` : ""}`],
          }));
        } else if (ev.event === "done") {
          setOcr((s) => ({ ...s, log: [...s.log, `完成：${ev.pages_ok} 成功 / ${ev.pages_failed} 失败`] }));
        }
      }),
      onOcrDone((r) => {
        if (r.task_id !== ocrIdRef.current) return;
        setOcr((s) => ({ ...s, taskId: null, result: r }));
        message.success(`导入完成：${r.paragraphs} 段落`);
      }),
      onOcrError((e) => {
        if (e.task_id !== ocrIdRef.current) return;
        setOcr((s) => ({ ...s, taskId: null }));
        message.error(`导入失败：${e.message}`);
      }),

      // ---- Translation pipeline ----
      onTranslationProgress((p) => {
        // Accept events while we track no task id (recovers a run started on
        // another page); the backend runs one pipeline at a time, so this is safe.
        if (transIdRef.current != null && p.task_id !== transIdRef.current) return;
        const ev = p.event;
        if (
          ev.event === "tu_done" ||
          ev.event === "tu_failed" ||
          ev.event === "finished" ||
          ev.event === "stopped"
        ) {
          setTranslation((s) => ({ ...s, revision: s.revision + 1 }));
        }
      }),
      onTranslationDone((r) => {
        if (transIdRef.current != null && r.task_id !== transIdRef.current) return;
        setTranslation((s) => ({ ...s, running: false, taskId: null, revision: s.revision + 1 }));
        message.success("翻译完成");
      }),
      onTranslationError((e) => {
        if (transIdRef.current != null && e.task_id !== transIdRef.current) return;
        setTranslation((s) => ({ ...s, running: false, taskId: null, revision: s.revision + 1 }));
        message.error(`翻译出错：${e.message}`);
      }),

      // ---- Project export ----
      onExportProgress((p) => {
        // Accept events while we track no task id: a tiny project can finish
        // packing before `exportStart` commits its id, so a fast `start` event
        // would otherwise be dropped and `busy` stuck forever.
        if (exportIdRef.current != null && p.task_id !== exportIdRef.current) return;
        const ev = p.event;
        if (ev.event === "start") setExportTask((s) => ({ ...s, progress: null }));
        else if (ev.event === "progress")
          setExportTask((s) => ({ ...s, progress: { done: ev.done, total: ev.total_files } }));
      }),
      onExportDone((r) => {
        if (exportIdRef.current != null && r.task_id !== exportIdRef.current) return;
        // `taskId: null` (not `r.task_id`) so a subsequent export sees a clean
        // slate; the accept-when-null guard above already let a fast done event
        // through, so the id is only used for filtering, never stored stale.
        setExportTask((s) => ({ ...s, taskId: null, busy: false, progress: null, result: r }));
        message.success("项目已导出");
      }),
      onExportError((e) => {
        if (exportIdRef.current != null && e.task_id !== exportIdRef.current) return;
        setExportTask((s) => ({ ...s, taskId: null, busy: false, progress: null }));
        message.error(`导出失败：${e.message}`);
      }),
    ];
    return () => {
      subs.forEach((u) => void u.then((f) => f()));
    };
  }, [message]);

  const value = useMemo<TasksContextValue>(
    () => ({
      ocr,
      translation,
      exportTask,
      ocrStart,
      ocrCancel,
      ocrClear,
      translationStart,
      translationSync,
      translationStop,
      translationClear,
      exportStart,
      exportClear,
      clearAll,
    }),
    [
      ocr,
      translation,
      exportTask,
      ocrStart,
      ocrCancel,
      ocrClear,
      translationStart,
      translationSync,
      translationStop,
      translationClear,
      exportStart,
      exportClear,
      clearAll,
    ],
  );

  return <TasksContext.Provider value={value}>{children}</TasksContext.Provider>;
}
