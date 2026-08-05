import { useEffect, useRef, useState } from "react";
import { App as AntdApp, Button, Card, Input, Progress, Space, Tag, Typography } from "antd";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { ImportResult, ProgressPayload } from "../types";
import { api, onOcrDone, onOcrError, onOcrProgress } from "../api";

export default function ImportPage({ onImported }: { onImported?: () => void }) {
  const { message } = AntdApp.useApp();
  const [input, setInput] = useState("");
  const [pages, setPages] = useState("");
  const [txtPath, setTxtPath] = useState("");
  const [taskId, setTaskId] = useState<string | null>(null);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [log, setLog] = useState<string[]>([]);
  const [result, setResult] = useState<ImportResult | null>(null);
  const taskRef = useRef<string | null>(null);

  useEffect(() => {
    taskRef.current = taskId;
  }, [taskId]);

  useEffect(() => {
    const subs: Promise<UnlistenFn>[] = [
      onOcrProgress((p: ProgressPayload) => {
        if (p.task_id !== taskRef.current) return;
        const ev = p.event;
        if (ev.event === "start") setLog((l) => [...l, `开始：共 ${ev.pages_total} 页`]);
        else if (ev.event === "page") {
          setProgress({ done: ev.done, total: ev.total });
          setLog((l) => [...l, `第 ${ev.page} 页：${ev.status}${ev.error ? ` (${ev.error})` : ""}`]);
        } else if (ev.event === "done") {
          setLog((l) => [...l, `完成：${ev.pages_ok} 成功 / ${ev.pages_failed} 失败`]);
        }
      }),
      onOcrDone((r) => {
        if (r.task_id !== taskRef.current) return;
        setResult(r);
        setTaskId(null);
        message.success(`导入完成：${r.paragraphs} 段落`);
        onImported?.();
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
  }, [message, onImported]);

  const startOcr = async () => {
    if (!input.trim()) {
      message.warning("请输入文件路径");
      return;
    }
    setResult(null);
    setLog([]);
    setProgress(null);
    try {
      setTaskId(await api.importOcr(input.trim(), pages.trim() || undefined));
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

  const importTxt = async () => {
    if (!txtPath.trim()) {
      message.warning("请输入 txt 路径");
      return;
    }
    try {
      const r = await api.importTxtFile(txtPath.trim());
      message.success(`txt 导入完成：${r.paragraphs} 段落`);
      onImported?.();
    } catch (e) {
      message.error(String(e));
    }
  };

  const outcomeColor = (o: string) => (o === "all_ok" ? "green" : o === "partial" ? "orange" : "red");

  return (
    <Space direction="vertical" size="large" style={{ width: "100%", maxWidth: 800 }}>
      <Card title="OCR 导入（PDF / 图片 / 目录）">
        <Space direction="vertical" style={{ width: "100%" }}>
          <Input
            addonBefore="路径"
            placeholder="/path/to/book.pdf"
            value={input}
            onChange={(e) => setInput(e.target.value)}
          />
          <Input
            addonBefore="页码"
            placeholder="可选，如 3,7,10-12（留空 = 全部）"
            value={pages}
            onChange={(e) => setPages(e.target.value)}
          />
          <Space>
            <Button type="primary" onClick={startOcr} disabled={!!taskId}>
              开始 OCR 导入
            </Button>
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
              结果：{result.outcome}，{result.paragraphs} 段落，失败页 [{result.failed_pages.join(", ")}]
            </Tag>
          )}
          {log.length > 0 && (
            <Card size="small" styles={{ body: { maxHeight: 180, overflow: "auto", fontFamily: "monospace", fontSize: 12 } }}>
              {log.map((l, i) => (
                <div key={i}>{l}</div>
              ))}
            </Card>
          )}
        </Space>
      </Card>

      {/* txt import auto-detects encoding (UTF-8/Shift-JIS/EUC-JP/UTF-16). */}
      <Card title="文本导入（txt）">
        <Space.Compact style={{ width: "100%" }}>
          <Input
            placeholder="/path/to/novel.txt"
            value={txtPath}
            onChange={(e) => setTxtPath(e.target.value)}
            onPressEnter={importTxt}
          />
          <Button onClick={importTxt}>导入 txt</Button>
        </Space.Compact>
        {/* A native file picker (tauri-plugin-dialog) will replace the path box later. */}
        <Typography.Paragraph type="secondary" style={{ marginTop: 8, marginBottom: 0 }}>
          请粘贴文件的绝对路径。
        </Typography.Paragraph>
      </Card>
    </Space>
  );
}
