import { useEffect, useRef, useState } from "react";
import {
  App as AntdApp,
  Button,
  Card,
  Checkbox,
  Input,
  InputNumber,
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
  ExtractedName,
  FileSelection,
  ImageMatchRule,
  ImportResult,
  ProgressPayload,
} from "../types";
import { DEFAULT_IMAGE_RULE } from "../types";
import { api, onOcrDone, onOcrError, onOcrProgress } from "../api";

/// Confirmation target for extracted candidates / CSV rows.
type GlossaryTarget = "project" | "global";

function outcomeColor(o: string): string {
  return o === "all_ok" ? "green" : o === "partial" ? "orange" : "red";
}

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
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [log, setLog] = useState<string[]>([]);
  const [result, setResult] = useState<ImportResult | null>(null);

  // ---- ③ 专名抽取与校对 ----------------------------------------------------
  const [extracting, setExtracting] = useState(false);
  const [candidates, setCandidates] = useState<ExtractedName[]>([]);
  const [confirmTarget, setConfirmTarget] = useState<GlossaryTarget>("project");

  // ---- ④ 专名 CSV 导入 ------------------------------------------------------
  const [csvPath, setCsvPath] = useState("");
  const [csvHeaders, setCsvHeaders] = useState<string[]>([]);
  const [jp, setJp] = useState(0);
  const [zh, setZh] = useState(1);
  const [en, setEn] = useState<number | null>(2);
  const [al, setAl] = useState<number | null>(3);
  const [hasHeader, setHasHeader] = useState(true);

  useEffect(() => {
    taskRef.current = taskId;
  }, [taskId]);

  useEffect(() => {
    const subs: Promise<UnlistenFn>[] = [
      onOcrProgress((p: ProgressPayload) => {
        if (p.task_id !== taskRef.current) return;
        const ev = p.event;
        if (ev.event === "start") setLog((l) => [...l, `开始：共 ${ev.pages_total} 项`]);
        else if (ev.event === "page") {
          setProgress({ done: ev.done, total: ev.total });
          setLog((l) => [...l, `第 ${ev.page} 项：${ev.status}${ev.error ? ` (${ev.error})` : ""}`]);
        } else if (ev.event === "done") {
          setLog((l) => [...l, `完成：${ev.pages_ok} 成功 / ${ev.pages_failed} 失败`]);
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
      message.success(confirmTarget === "project" ? "已通过进项目小词库" : "已通过进全局大词库");
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

  const previewCsv = async () => {
    if (!csvPath.trim()) {
      message.warning("请输入 CSV 路径");
      return;
    }
    try {
      setCsvHeaders(await api.csvHeaders(csvPath.trim()));
    } catch (e) {
      message.error(String(e));
    }
  };

  const importCsv = async () => {
    if (!csvPath.trim()) {
      message.warning("请输入 CSV 路径");
      return;
    }
    try {
      const n = await api.importGlossaryCsv(
        csvPath.trim(),
        {
          japanese: jp,
          chinese: zh,
          english: en ?? undefined,
          aliases: al ?? undefined,
          has_header: hasHeader,
        },
        confirmTarget,
      );
      message.success(`导入 ${n} 条`);
      setCsvPath("");
    } catch (e) {
      message.error(String(e));
    }
  };

  const candidateColumns: TableProps<ExtractedName>["columns"] = [
    { title: "日文", dataIndex: "japanese", width: 220 },
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
              api.updateExtracted(r.id, val).catch((err) => message.error(String(err)));
            }
          }}
        />
      ),
    },
    {
      title: "操作",
      width: 150,
      render: (_: unknown, r) => (
        <Space>
          <Button size="small" type="link" onClick={() => confirmCandidate(r.id)}>
            通过
          </Button>
          <Button size="small" type="link" danger onClick={() => rejectCandidate(r.id)}>
            拒绝
          </Button>
        </Space>
      ),
    },
  ];

  return (
    <Space direction="vertical" size="large" style={{ width: "100%", maxWidth: 900 }}>
      {/* ① 文本导入：txt / md 直接读入，编码自动检测，源文件原地读取。 */}
      <Card title="文本导入（txt / md）">
        <Space.Compact style={{ width: "100%" }}>
          <Input
            placeholder="/path/to/novel.txt"
            value={txtPath}
            onChange={(e) => setTxtPath(e.target.value)}
            onPressEnter={importTxt}
          />
          <Button onClick={importTxt}>导入文本</Button>
        </Space.Compact>
        <Typography.Paragraph type="secondary" style={{ marginTop: 8, marginBottom: 0 }}>
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
              <Input
                addonBefore="文件"
                placeholder="/path/to/book.pdf"
                value={pdfPath}
                onChange={(e) => setPdfPath(e.target.value)}
              />
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
              <Input
                addonBefore="目录"
                placeholder="/path/to/pages"
                value={imgDir}
                onChange={(e) => setImgDir(e.target.value)}
              />
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
                    setRule({ ...rule, range: [v ?? 1, rule.range?.[1] ?? v ?? 1] })
                  }
                />
                <InputNumber
                  addonBefore="止"
                  min={1}
                  value={rule.range?.[1] ?? undefined}
                  onChange={(v) =>
                    setRule({ ...rule, range: [rule.range?.[0] ?? v ?? 1, v ?? 1] })
                  }
                />
                <Button onClick={() => setRule({ ...rule, range: null })}>清除范围</Button>
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
                    style={{ marginBottom: 0, fontFamily: "monospace", fontSize: 12 }}
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
              <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>
                目录内混入的 PDF 会被跳过（非预期输入）。
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
              percent={progress.total ? Math.round((progress.done / progress.total) * 100) : 0}
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
              styles={{ body: { maxHeight: 180, overflow: "auto", fontFamily: "monospace", fontSize: 12 } }}
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
        <Table
          rowKey="id"
          size="small"
          columns={candidateColumns}
          dataSource={candidates}
          pagination={{ pageSize: 10 }}
          locale={{ emptyText: "暂无候选，先运行抽取" }}
        />
      </Card>

      {/* ④ 专名 CSV 导入：表头预览 + 列选择 + 目标词库。 */}
      <Card title="专名 CSV 导入">
        <Space direction="vertical" style={{ width: "100%" }}>
          <Space.Compact style={{ width: "100%" }}>
            <Input
              placeholder="/path/to/glossary.csv"
              value={csvPath}
              onChange={(e) => setCsvPath(e.target.value)}
              onPressEnter={importCsv}
            />
            <Button onClick={previewCsv}>预览表头</Button>
            <Button type="primary" onClick={importCsv}>
              导入
            </Button>
          </Space.Compact>
          {csvHeaders.length > 0 && (
            <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>
              表头：{csvHeaders.map((h, i) => `${i}:${h}`).join("　")}
            </Typography.Paragraph>
          )}
          <Space wrap>
            <InputNumber addonBefore="日文列" min={0} value={jp} onChange={(v) => setJp(v ?? 0)} />
            <InputNumber addonBefore="中文列" min={0} value={zh} onChange={(v) => setZh(v ?? 1)} />
            <InputNumber addonBefore="英文列" min={0} value={en ?? undefined} onChange={(v) => setEn(v)} />
            <InputNumber addonBefore="别名列" min={0} value={al ?? undefined} onChange={(v) => setAl(v)} />
            <Checkbox checked={hasHeader} onChange={(e) => setHasHeader(e.target.checked)}>
              含表头
            </Checkbox>
          </Space>
        </Space>
      </Card>
    </Space>
  );
}
