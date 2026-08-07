import { useEffect, useRef, useState } from "react";
import {
  App as AntdApp,
  Button,
  Card,
  Checkbox,
  Input,
  InputNumber,
  Modal,
  Progress,
  Segmented,
  Select,
  Space,
  Table,
  Tag,
  Typography,
  type TableProps,
} from "antd";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type {
  CsvMapping,
  CsvPreviewRow,
  ExtractedName,
  FileSelection,
  ImageMatchRule,
  ImportResult,
  ProgressPayload,
} from "../types";
import { DEFAULT_IMAGE_RULE } from "../types";
import { api, onOcrDone, onOcrError, onOcrProgress } from "../api";
import { pickDirectory, pickFile } from "../dialog";

/// Confirmation target for extracted candidates / CSV rows.
type GlossaryTarget = "project" | "global";

function outcomeColor(o: string): string {
  return o === "all_ok" ? "green" : o === "partial" ? "orange" : "red";
}

/// Sentinel: a CSV column index of `-1` means "不使用该列"（导入时丢弃）。
const CSV_NOT_USED = -1;

/// CSV 业务字段 → 列映射 UI 行配置（`required` = 必填字段，其下拉不含「不使用」）。
const CSV_FIELD_ROWS: {
  key: "japanese" | "chinese" | "english" | "category" | "notes";
  label: string;
  required: boolean;
}[] = [
  { key: "japanese", label: "日文", required: true },
  { key: "chinese", label: "中文", required: true },
  { key: "english", label: "英文", required: false },
  { key: "category", label: "分类", required: false },
  { key: "notes", label: "备注", required: false },
];

// Mirrors the `category` values the extraction / auto-tag prompts may output —
// offered as quick-picks (the tags Select also allows free entry).
const TAG_OPTIONS = ["人名", "地名", "组织", "作品名", "物品", "系统", "术语", "其他"].map(
  (t) => ({ value: t, label: t }),
);

export default function ImportPage() {
  const { message } = AntdApp.useApp();
  const taskRef = useRef<string | null>(null);

  // ---- ① 文本导入 (txt/md) ------------------------------------------------
  const [txtPath, setTxtPath] = useState("");

  // ---- ② OCR：PDF（单文件） / 图片目录（batch） -----------------------------
  const [ocrMode, setOcrMode] = useState<"pdf" | "images">("pdf");
  const [pdfPath, setPdfPath] = useState("");
  const [pages, setPages] = useState("");
  const [imgDir, setImgDir] = useState("");
  const [rule, setRule] = useState<ImageMatchRule>({ ...DEFAULT_IMAGE_RULE });
  const [selection, setSelection] = useState<FileSelection | null>(null);
  const [scanning, setScanning] = useState(false);
  const [taskId, setTaskId] = useState<string | null>(null);
  const [progress, setProgress] = useState<{
    done: number;
    total: number;
  } | null>(null);
  const [log, setLog] = useState<string[]>([]);
  const [result, setResult] = useState<ImportResult | null>(null);

  // ---- ③ 专名抽取与校对 ----------------------------------------------------
  const [extracting, setExtracting] = useState(false);
  const [candidates, setCandidates] = useState<ExtractedName[]>([]);
  const [confirmTarget, setConfirmTarget] = useState<GlossaryTarget>("project");
  const [selectedCand, setSelectedCand] = useState<React.Key[]>([]);
  const [autoTagging, setAutoTagging] = useState(false);
  // 批量标记 Modal：一次性给所有勾选的候选打上同一个标签。
  const [batchTagOpen, setBatchTagOpen] = useState(false);
  const [batchTag, setBatchTag] = useState<string[]>([]);

  // ---- ④ 专名 CSV 导入 ------------------------------------------------------
  const [csvPath, setCsvPath] = useState("");
  const [csvHeaders, setCsvHeaders] = useState<string[]>([]);
  // CSV 列映射：每个业务字段对应一个列索引；CSV_NOT_USED(-1) = 不使用该列（丢弃）。
  const [csvMap, setCsvMap] = useState({
    japanese: 0,
    chinese: 1,
    english: 2,
    category: CSV_NOT_USED,
    notes: CSV_NOT_USED,
  });
  const [hasHeader, setHasHeader] = useState(true);
  const [csvPreviewRows, setCsvPreviewRows] = useState<CsvPreviewRow[]>([]);
  const [csvPreviewErr, setCsvPreviewErr] = useState("");

  // CSV 列映射辅助：下拉选项、已使用/丢弃列、预览列定义、invoke payload。
  const csvColumnOptions = csvHeaders.map((h, i) => ({
    value: i,
    label: hasHeader ? `${h}（第 ${i + 1} 列）` : `第 ${i + 1} 列`,
  }));
  const usedColumnSet = new Set(
    [
      csvMap.japanese,
      csvMap.chinese,
      csvMap.english,
      csvMap.category,
      csvMap.notes,
    ].filter((i) => i >= 0),
  );
  const discardedColumns = csvHeaders
    .map((_, i) => i)
    .filter((i) => !usedColumnSet.has(i))
    .map((i) =>
      hasHeader ? `第 ${i + 1} 列（${csvHeaders[i]}）` : `第 ${i + 1} 列`,
    );
  const csvMappingPayload = (): CsvMapping => ({
    japanese: csvMap.japanese,
    chinese: csvMap.chinese,
    english: csvMap.english >= 0 ? csvMap.english : null,
    category: csvMap.category >= 0 ? csvMap.category : null,
    notes: csvMap.notes >= 0 ? csvMap.notes : null,
    has_header: hasHeader,
  });
  const csvPreviewColumns: TableProps<CsvPreviewRow>["columns"] = [
    { title: "日文", dataIndex: "japanese" },
    { title: "中文", dataIndex: "chinese" },
    ...(csvMap.english >= 0
      ? [
          {
            title: "英文",
            dataIndex: "english",
            render: (v: string | null) => v ?? "—",
          },
        ]
      : []),
    ...(csvMap.category >= 0
      ? [
          {
            title: "分类",
            dataIndex: "category",
            render: (v: string | null) => v ?? "—",
          },
        ]
      : []),
    ...(csvMap.notes >= 0
      ? [
          {
            title: "备注",
            dataIndex: "notes",
            render: (v: string | null) => v ?? "—",
          },
        ]
      : []),
  ];

  // 按当前列映射自动刷新前几行解析预览（防抖，避免每次下拉改动都重新解析整个文件）。
  useEffect(() => {
    if (!csvPath.trim() || csvHeaders.length === 0) {
      setCsvPreviewRows([]);
      setCsvPreviewErr("");
      return;
    }
    const mapping: CsvMapping = {
      japanese: csvMap.japanese,
      chinese: csvMap.chinese,
      english: csvMap.english >= 0 ? csvMap.english : null,
      category: csvMap.category >= 0 ? csvMap.category : null,
      notes: csvMap.notes >= 0 ? csvMap.notes : null,
      has_header: hasHeader,
    };
    const t = setTimeout(() => {
      api
        .csvPreview(csvPath.trim(), mapping, 5)
        .then((rows) => {
          setCsvPreviewRows(rows);
          setCsvPreviewErr("");
        })
        .catch((e) => {
          setCsvPreviewRows([]);
          setCsvPreviewErr(String(e));
        });
    }, 250);
    return () => clearTimeout(t);
  }, [csvPath, csvHeaders, csvMap, hasHeader]);

  useEffect(() => {
    taskRef.current = taskId;
  }, [taskId]);

  useEffect(() => {
    const subs: Promise<UnlistenFn>[] = [
      onOcrProgress((p: ProgressPayload) => {
        if (p.task_id !== taskRef.current) return;
        const ev = p.event;
        if (ev.event === "start")
          setLog((l) => [...l, `开始：共 ${ev.pages_total} 项`]);
        else if (ev.event === "page") {
          setProgress({ done: ev.done, total: ev.total });
          setLog((l) => [
            ...l,
            `第 ${ev.page} 项：${ev.status}${ev.error ? ` (${ev.error})` : ""}`,
          ]);
        } else if (ev.event === "done") {
          setLog((l) => [
            ...l,
            `完成：${ev.pages_ok} 成功 / ${ev.pages_failed} 失败`,
          ]);
        }
      }),
      onOcrDone((r) => {
        if (r.task_id !== taskRef.current) return;
        setResult(r);
        setTaskId(null);
        message.success(`导入完成：${r.paragraphs} 段落`);
      }),
      onOcrError((e) => {
        if (e.task_id !== taskRef.current) return;
        setTaskId(null);
        message.error(`导入失败：${e.message}`);
      }),
    ];
    return () => {
      subs.forEach((u) => void u.then((f) => f()));
    };
  }, [message]);

  const importTxt = async () => {
    if (!txtPath.trim()) {
      message.warning("请输入文本文件路径");
      return;
    }
    try {
      const r = await api.importTxtFile(txtPath.trim());
      message.success(`导入完成：${r.paragraphs} 段落`);
    } catch (e) {
      message.error(String(e));
    }
  };

  const startPdf = async () => {
    if (!pdfPath.trim()) {
      message.warning("请输入 PDF / 图片文件路径");
      return;
    }
    setResult(null);
    setLog([]);
    setProgress(null);
    try {
      setTaskId(await api.importOcr(pdfPath.trim(), pages.trim() || undefined));
    } catch (e) {
      message.error(String(e));
    }
  };

  const scanDir = async () => {
    if (!imgDir.trim()) {
      message.warning("请输入图片目录路径");
      return;
    }
    setScanning(true);
    try {
      setSelection(await api.scanImageDir(imgDir.trim(), rule));
    } catch (e) {
      setSelection(null);
      message.error(String(e));
    } finally {
      setScanning(false);
    }
  };

  const startBatch = async () => {
    if (!imgDir.trim()) {
      message.warning("请输入图片目录路径");
      return;
    }
    if (!selection || selection.matched === 0) {
      message.warning("请先扫描确认有匹配的图片");
      return;
    }
    setResult(null);
    setLog([]);
    setProgress(null);
    try {
      setTaskId(await api.importImagesBatch(imgDir.trim(), rule));
    } catch (e) {
      message.error(String(e));
    }
  };

  const cancel = async () => {
    if (!taskId) return;
    try {
      await api.cancelImport(taskId);
      message.info("已请求取消");
    } catch (e) {
      message.error(String(e));
    }
  };

  const runExtract = async () => {
    setExtracting(true);
    try {
      const n = await api.runNameExtraction();
      message.success(`新增候选 ${n} 条`);
      await loadCandidates();
    } catch (e) {
      message.error(String(e));
    } finally {
      setExtracting(false);
    }
  };

  const loadCandidates = async () => {
    try {
      setCandidates(await api.listExtracted("new"));
    } catch (e) {
      message.error(String(e));
    }
  };

  useEffect(() => {
    void loadCandidates();
  }, []);

  const confirmCandidate = async (id: number) => {
    try {
      await api.confirmExtracted(id, confirmTarget);
      await loadCandidates();
      message.success(
        confirmTarget === "project"
          ? "已通过进项目小词库"
          : "已通过进全局大词库",
      );
    } catch (e) {
      message.error(String(e));
    }
  };

  const rejectCandidate = async (id: number) => {
    try {
      await api.rejectExtracted(id);
      await loadCandidates();
    } catch (e) {
      message.error(String(e));
    }
  };

  const confirmCandidates = async (target: "project" | "global") => {
    const ids = selectedCand.map(Number);
    if (ids.length === 0) return;
    try {
      const n = await api.confirmExtractedBatch(ids, target);
      message.success(
        target === "project"
          ? `已确认 ${n} 条进项目小词库`
          : `已确认 ${n} 条进全局大词库`,
      );
      setSelectedCand([]);
      await loadCandidates();
    } catch (e) {
      message.error(String(e));
    }
  };

  const rejectCandidates = async () => {
    const ids = selectedCand.map(Number);
    if (ids.length === 0) return;
    try {
      const n = await api.rejectExtractedBatch(ids);
      message.success(`已拒绝 ${n} 条`);
      setSelectedCand([]);
      await loadCandidates();
    } catch (e) {
      message.error(String(e));
    }
  };

  /// 手动触发：LLM 给勾选的候选自动打标签（每个候选只写第一个分类，已有标签不覆盖）。
  const autoTag = async () => {
    const ids = selectedCand.map(Number);
    if (ids.length === 0) return;
    setAutoTagging(true);
    try {
      const n = await api.autoTagExtracted(ids);
      message.success(n > 0 ? `已自动打标签 ${n} 条` : "没有可打标签的候选（检查设置页「打标签 Prompt」）");
      await loadCandidates();
    } catch (e) {
      message.error(String(e));
    } finally {
      setAutoTagging(false);
    }
  };

  /// 批量标记：给勾选的候选全部写入同一个标签（复选框 + 全选 + 批量确认共用选择集）。
  const applyBatchTag = async () => {
    const ids = selectedCand.map(Number);
    if (ids.length === 0) return;
    try {
      const n = await api.applyExtractedTags(ids, batchTag);
      message.success(`已标记 ${n} 条`);
      setBatchTagOpen(false);
      setBatchTag([]);
      setSelectedCand([]);
      await loadCandidates();
    } catch (e) {
      message.error(String(e));
    }
  };

  const openBatchTag = () => {
    if (selectedCand.length === 0) return;
    setBatchTag([]);
    setBatchTagOpen(true);
  };

  const setCandidateTags = async (id: number, tags: string[]) => {
    try {
      await api.updateExtractedTags(id, tags);
      // Keep the local candidate list in sync so the controlled tags Select
      // doesn't snap back to the stale server value on the next re-render.
      setCandidates((prev) => prev.map((c) => (c.id === id ? { ...c, tags } : c)));
    } catch (e) {
      message.error(String(e));
    }
  };

  const previewCsv = async () => {
    if (!csvPath.trim()) {
      message.warning("请输入 CSV 路径");
      return;
    }
    try {
      const headers = await api.csvHeaders(csvPath.trim());
      setCsvHeaders(headers);
      // 保留仍然有效的列选择；越界的自动回落到合理默认值（切文件后下拉总有真实选项）。
      setCsvMap((m) => ({
        japanese: m.japanese < headers.length ? m.japanese : 0,
        chinese: m.chinese < headers.length ? m.chinese : 1,
        english:
          m.english >= 0 && m.english < headers.length
            ? m.english
            : headers.length > 2
              ? 2
              : CSV_NOT_USED,
        category:
          m.category >= 0 && m.category < headers.length
            ? m.category
            : CSV_NOT_USED,
        notes:
          m.notes >= 0 && m.notes < headers.length ? m.notes : CSV_NOT_USED,
      }));
    } catch (e) {
      message.error(String(e));
    }
  };

  const importCsv = async () => {
    if (!csvPath.trim()) {
      message.warning("请输入 CSV 路径");
      return;
    }
    if (csvMap.japanese === CSV_NOT_USED || csvMap.chinese === CSV_NOT_USED) {
      message.warning("日文列与中文列为必填，请分别选择对应列");
      return;
    }
    try {
      const n = await api.importGlossaryCsv(
        csvPath.trim(),
        csvMappingPayload(),
        confirmTarget,
      );
      message.success(`导入 ${n} 条`);
      setCsvPath("");
      setCsvPreviewRows([]);
      setCsvPreviewErr("");
    } catch (e) {
      message.error(String(e));
    }
  };

  const candidateColumns: TableProps<ExtractedName>["columns"] = [
    {
      title: "日文（可改）",
      dataIndex: "japanese",
      width: 220,
      render: (v: string, r) => (
        <Input
          size="small"
          defaultValue={v}
          onBlur={(e) => {
            const val = e.target.value.trim();
            if (val !== v) {
              api
                .updateExtractedJapanese(r.id, val)
                .then(() => {
                  message.success("日文已修正");
                  // Refresh so the row's stored value matches (also clears any
                  // stale rendered form from a failed rename).
                  void loadCandidates();
                })
                .catch((err) => {
                  message.error(String(err));
                  // Reset the input to the stored value so the cell never shows
                  // a japanese the DB doesn't have.
                  e.target.value = v;
                });
            }
          }}
        />
      ),
    },
    {
      title: "中文（可改）",
      dataIndex: "candidate_chinese",
      render: (v: string | null, r) => (
        <Input
          size="small"
          defaultValue={v ?? ""}
          onBlur={(e) => {
            const val = e.target.value;
            if (val !== (v ?? "")) {
              api
                .updateExtracted(r.id, val)
                .catch((err) => message.error(String(err)));
            }
          }}
        />
      ),
    },
    {
      title: "标签",
      dataIndex: "tags",
      width: 200,
      render: (v: string[], r) => (
        <Select
          size="small"
          mode="tags"
          style={{ width: "100%" }}
          value={v ?? []}
          placeholder="标签"
          options={TAG_OPTIONS}
          onChange={(tags: string[]) => void setCandidateTags(r.id, tags)}
        />
      ),
    },
    {
      title: "操作",
      width: 150,
      render: (_: unknown, r) => (
        <Space>
          <Button
            size="small"
            type="link"
            onClick={() => confirmCandidate(r.id)}
          >
            通过
          </Button>
          <Button
            size="small"
            type="link"
            danger
            onClick={() => rejectCandidate(r.id)}
          >
            拒绝
          </Button>
        </Space>
      ),
    },
  ];

  return (
    <Space
      direction="vertical"
      size="large"
      style={{ width: "100%", maxWidth: 900 }}
    >
      {/* ① 文本导入：txt / md 直接读入，编码自动检测，源文件原地读取。 */}
      <Card title="文本导入（txt / md）">
        <Space.Compact style={{ width: "100%" }}>
          <Input
            placeholder="/path/to/novel.txt"
            value={txtPath}
            onChange={(e) => setTxtPath(e.target.value)}
            onPressEnter={importTxt}
          />
          <Button
            onClick={async () => {
              const p = await pickFile({
                title: "选择文本文件",
                filters: [{ name: "文本", extensions: ["txt", "md"] }],
              });
              if (p) setTxtPath(p);
            }}
          >
            选择…
          </Button>
          <Button onClick={importTxt}>导入文本</Button>
        </Space.Compact>
        <Typography.Paragraph
          type="secondary"
          style={{ marginTop: 8, marginBottom: 0 }}
        >
          请粘贴文件的绝对路径（编码自动识别）。
        </Typography.Paragraph>
      </Card>

      {/* ② OCR：PDF（extract，保留页码）或 图片目录（batch + 匹配规则 + 预览）。 */}
      <Card title="OCR 导入">
        <Space direction="vertical" size="middle" style={{ width: "100%" }}>
          <Segmented
            value={ocrMode}
            onChange={(v) => setOcrMode(v as "pdf" | "images")}
            options={[
              { label: "PDF / 单图", value: "pdf" },
              { label: "图片目录", value: "images" },
            ]}
          />
          {ocrMode === "pdf" ? (
            <Space direction="vertical" style={{ width: "100%" }}>
              <Space.Compact style={{ width: "100%" }}>
                <Input
                  addonBefore="文件"
                  placeholder="/path/to/book.pdf"
                  value={pdfPath}
                  onChange={(e) => setPdfPath(e.target.value)}
                />
                <Button
                  onClick={async () => {
                    const p = await pickFile({
                      title: "选择 PDF / 图片文件",
                      filters: [
                        { name: "文档", extensions: ["pdf"] },
                        { name: "图片", extensions: ["png", "jpg", "jpeg", "webp"] },
                      ],
                    });
                    if (p) setPdfPath(p);
                  }}
                >
                  选择…
                </Button>
              </Space.Compact>
              <Input
                addonBefore="页码"
                placeholder="可选，如 3,7,10-12（留空 = 全部）"
                value={pages}
                onChange={(e) => setPages(e.target.value)}
              />
              <Space>
                <Button type="primary" onClick={startPdf} disabled={!!taskId}>
                  开始 OCR
                </Button>
              </Space>
            </Space>
          ) : (
            <Space direction="vertical" style={{ width: "100%" }}>
              <Space.Compact style={{ width: "100%" }}>
                <Input
                  addonBefore="目录"
                  placeholder="/path/to/pages"
                  value={imgDir}
                  onChange={(e) => setImgDir(e.target.value)}
                />
                <Button
                  onClick={async () => {
                    const p = await pickDirectory({ title: "选择图片目录" });
                    if (p) setImgDir(p);
                  }}
                >
                  选择…
                </Button>
              </Space.Compact>
              <Space wrap>
                <Select
                  style={{ width: 200 }}
                  value={rule.preset}
                  onChange={(v) => setRule({ ...rule, preset: v })}
                  options={[
                    { value: "all", label: "全部图片" },
                    { value: "png", label: "仅 PNG" },
                    { value: "jpg", label: "仅 JPG/JPEG" },
                    { value: "numbered", label: "纯数字文件名" },
                    { value: "numbered_prefix", label: "数字开头文件名" },
                  ]}
                />
                <Input
                  addonBefore="自定义匹配"
                  placeholder="可选 glob，如 *.png"
                  value={rule.custom_glob ?? ""}
                  onChange={(e) =>
                    setRule({ ...rule, custom_glob: e.target.value || null })
                  }
                  style={{ width: 220 }}
                />
                <InputNumber
                  addonBefore="范围"
                  placeholder="起"
                  min={1}
                  value={rule.range?.[0] ?? undefined}
                  onChange={(v) =>
                    setRule({
                      ...rule,
                      range: [v ?? 1, rule.range?.[1] ?? v ?? 1],
                    })
                  }
                />
                <InputNumber
                  addonBefore="止"
                  min={1}
                  value={rule.range?.[1] ?? undefined}
                  onChange={(v) =>
                    setRule({
                      ...rule,
                      range: [rule.range?.[0] ?? v ?? 1, v ?? 1],
                    })
                  }
                />
                <Button onClick={() => setRule({ ...rule, range: null })}>
                  清除范围
                </Button>
                <Button loading={scanning} onClick={scanDir}>
                  扫描预览
                </Button>
              </Space>
              {selection && (
                <Tag color={selection.matched > 0 ? "green" : "red"}>
                  命中 {selection.matched}/{selection.total}，共{" "}
                  {(selection.bytes / 1024).toFixed(0)} KB
                </Tag>
              )}
              {selection && selection.matched > 0 && (
                <Card
                  size="small"
                  title={`已匹配 ${selection.matched} 张`}
                  styles={{ body: { maxHeight: 180, overflow: "auto" } }}
                >
                  <Typography.Paragraph
                    style={{
                      marginBottom: 0,
                      fontFamily: "monospace",
                      fontSize: 12,
                    }}
                  >
                    {selection.names.map((n) => (
                      <div key={n}>{n}</div>
                    ))}
                  </Typography.Paragraph>
                </Card>
              )}
              <Space>
                <Button
                  type="primary"
                  onClick={startBatch}
                  disabled={!!taskId || !selection || selection.matched === 0}
                >
                  导入 {selection?.matched ?? 0} 张图片
                </Button>
              </Space>
              <Typography.Paragraph
                type="secondary"
                style={{ marginBottom: 0 }}
              >
                目录内混入的 PDF 会被跳过。
              </Typography.Paragraph>
            </Space>
          )}

          {/* 共享的导入进度 / 日志 / 取消 */}
          <Space>
            <Button danger onClick={cancel} disabled={!taskId}>
              取消
            </Button>
          </Space>
          {progress && (
            <Progress
              percent={
                progress.total
                  ? Math.round((progress.done / progress.total) * 100)
                  : 0
              }
              status={taskId ? "active" : "normal"}
            />
          )}
          {result && (
            <Tag color={outcomeColor(result.outcome)}>
              结果：{result.outcome}，{result.paragraphs} 段落，失败项 [
              {result.failed_pages.join(", ")}]
            </Tag>
          )}
          {log.length > 0 && (
            <Card
              size="small"
              styles={{
                body: {
                  maxHeight: 180,
                  overflow: "auto",
                  fontFamily: "monospace",
                  fontSize: 12,
                },
              }}
            >
              {log.map((l, i) => (
                <div key={i}>{l}</div>
              ))}
            </Card>
          )}
        </Space>
      </Card>

      {/* ③ 专名抽取与校对：从 OCR 文本提取 → 人工校对 → 通过进词库。 */}
      <Card
        title="专名抽取与校对"
        extra={
          <Space>
            <span>通过目标：</span>
            <Select
              style={{ width: 160 }}
              value={confirmTarget}
              onChange={(v) => setConfirmTarget(v)}
              options={[
                { value: "project", label: "项目小词库" },
                { value: "global", label: "全局大词库" },
              ]}
            />
            <Button type="primary" loading={extracting} onClick={runExtract}>
              运行抽取
            </Button>
          </Space>
        }
      >
        <Space wrap style={{ marginBottom: 8 }}>
          <Button
            size="small"
            disabled={selectedCand.length === 0}
            onClick={() => confirmCandidates("project")}
          >
            确认所选进小词库
          </Button>
          <Button
            size="small"
            disabled={selectedCand.length === 0}
            onClick={() => confirmCandidates("global")}
          >
            确认所选进大词库
          </Button>
          <Button
            size="small"
            loading={autoTagging}
            disabled={selectedCand.length === 0}
            onClick={() => void autoTag()}
          >
            自动打标签
          </Button>
          <Button
            size="small"
            disabled={selectedCand.length === 0}
            onClick={openBatchTag}
          >
            批量标记
          </Button>
          <Button
            size="small"
            danger
            disabled={selectedCand.length === 0}
            onClick={() => rejectCandidates()}
          >
            拒绝所选
          </Button>
        </Space>
        <Table
          rowKey="id"
          size="small"
          columns={candidateColumns}
          dataSource={candidates}
          rowSelection={{
            selectedRowKeys: selectedCand,
            onChange: setSelectedCand,
          }}
          pagination={{ pageSize: 10 }}
          locale={{ emptyText: "暂无候选，先运行抽取" }}
        />
      </Card>

      {/* 批量标记：给勾选的候选一次性写入同一个标签。 */}
      <Modal
        title={`批量标记 ${selectedCand.length} 条候选`}
        open={batchTagOpen}
        onOk={() => void applyBatchTag()}
        onCancel={() => setBatchTagOpen(false)}
        okText="标记"
        cancelText="取消"
        okButtonProps={{ disabled: batchTag.length === 0 }}
      >
        <Space direction="vertical" style={{ width: "100%" }}>
          <Select
            mode="tags"
            style={{ width: "100%" }}
            placeholder="选择或输入标签（如：人名）"
            value={batchTag}
            onChange={(v: string[]) => setBatchTag(v)}
            options={TAG_OPTIONS}
          />
          <Typography.Text type="secondary">
            所有勾选的候选都会被写入该标签；已存在的标签会被覆盖。
          </Typography.Text>
        </Space>
      </Modal>

      {/* ④ 专名 CSV 导入：表头预览 + 逐字段列映射（未选列丢弃）+ 前几行预览 + 目标词库。 */}
      <Card title="专名 CSV 导入">
        <Space direction="vertical" style={{ width: "100%" }}>
          <Space.Compact style={{ width: "100%" }}>
            <Input
              placeholder="/path/to/glossary.csv"
              value={csvPath}
              onChange={(e) => setCsvPath(e.target.value)}
              onPressEnter={importCsv}
            />
            <Button
              onClick={async () => {
                const p = await pickFile({
                  title: "选择词库 CSV",
                  filters: [{ name: "CSV", extensions: ["csv"] }],
                });
                if (p) setCsvPath(p);
              }}
            >
              选择…
            </Button>
            <Button onClick={previewCsv}>预览表头</Button>
            <Button type="primary" onClick={importCsv}>
              导入
            </Button>
          </Space.Compact>
          {csvHeaders.length > 0 && (
            <>
              <div>
                <Typography.Text type="secondary">表头：</Typography.Text>
                {csvHeaders.map((h, i) => (
                  <Tag
                    key={i}
                    color={usedColumnSet.has(i) ? "blue" : "default"}
                    style={{ marginBottom: 4 }}
                  >
                    {hasHeader ? `${h}（第 ${i + 1} 列）` : `第 ${i + 1} 列`}
                  </Tag>
                ))}
              </div>
              <Typography.Paragraph
                type="secondary"
                style={{ marginBottom: 0 }}
              >
                蓝色 = 已映射；灰色 = 未选中，导入时丢弃。
              </Typography.Paragraph>

              <div>
                <Typography.Text strong>列映射</Typography.Text>
                <Typography.Text type="secondary">
                  {"　"}为每个业务字段选择对应的 CSV 列；未选中的列不会被导入。
                </Typography.Text>
              </div>
              {CSV_FIELD_ROWS.map((f) => (
                <Space key={f.key} wrap>
                  <Typography.Text style={{ width: 64 }}>
                    {f.label}
                    {f.required ? (
                      <span style={{ color: "#ff4d4f" }}>*</span>
                    ) : null}
                  </Typography.Text>
                  <Select
                    style={{ width: 300 }}
                    value={csvMap[f.key]}
                    onChange={(v) => setCsvMap((m) => ({ ...m, [f.key]: v }))}
                    options={
                      f.required
                        ? csvColumnOptions
                        : [
                            {
                              value: CSV_NOT_USED,
                              label: "不使用（丢弃该列）",
                            },
                            ...csvColumnOptions,
                          ]
                    }
                    placeholder={f.required ? "请选择列" : "不使用（可选）"}
                  />
                </Space>
              ))}
              {discardedColumns.length > 0 && (
                <Typography.Paragraph
                  type="secondary"
                  style={{ marginBottom: 0 }}
                >
                  将被丢弃的列：{discardedColumns.join("、")}
                </Typography.Paragraph>
              )}

              <Checkbox
                checked={hasHeader}
                onChange={(e) => setHasHeader(e.target.checked)}
              >
                含表头
              </Checkbox>

              <div>
                <Typography.Text strong>
                  导入预览（前 {csvPreviewRows.length} 行）
                </Typography.Text>
              </div>
              <Table
                size="small"
                rowKey={(_, index) => index ?? 0}
                pagination={false}
                dataSource={csvPreviewRows}
                locale={{
                  emptyText: csvPreviewErr
                    ? `预览失败：${csvPreviewErr}`
                    : "暂无预览数据",
                }}
                columns={csvPreviewColumns}
              />
            </>
          )}
        </Space>
      </Card>
    </Space>
  );
}
