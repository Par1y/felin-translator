import { useEffect, useRef, useState } from "react";
import {
  App as AntdApp,
  Alert,
  Button,
  Card,
  Input,
  Progress,
  Space,
  Typography,
} from "antd";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type {
  ExportProgressPayload,
  ExportResult,
  TranslationExport,
} from "../types";
import { api, onExportDone, onExportError, onExportProgress } from "../api";
import { pickDirectory, pickSavePath } from "../dialog";

export default function ExportPage() {
  const { message } = AntdApp.useApp();
  const [dest, setDest] = useState("");
  const [result, setResult] = useState<ExportResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{
    done: number;
    total: number;
  } | null>(null);
  const [txDir, setTxDir] = useState("");
  const [txResult, setTxResult] = useState<TranslationExport | null>(null);
  const [txBusy, setTxBusy] = useState(false);

  const taskRef = useRef<string | null>(null);

  useEffect(() => {
    const subs: Promise<UnlistenFn>[] = [
      onExportProgress((p: ExportProgressPayload) => {
        if (p.task_id !== taskRef.current) return;
        const ev = p.event;
        if (ev.event === "start") setProgress(null);
        else if (ev.event === "progress")
          setProgress({ done: ev.done, total: ev.total_files });
      }),
      onExportDone((r) => {
        if (r.task_id !== taskRef.current) return;
        taskRef.current = null;
        setBusy(false);
        setProgress(null);
        setResult(r);
        message.success("项目已导出");
      }),
      onExportError((e) => {
        if (e.task_id !== taskRef.current) return;
        taskRef.current = null;
        setBusy(false);
        setProgress(null);
        message.error(`导出失败：${e.message}`);
      }),
    ];
    return () => {
      subs.forEach((u) => void u.then((f) => f()));
    };
  }, [message]);

  const doExport = async () => {
    if (!dest.trim()) {
      message.warning("请输入导出归档的路径");
      return;
    }
    setResult(null);
    setProgress(null);
    setBusy(true);
    try {
      const r = await api.exportProject(dest.trim());
      taskRef.current = r.task_id;
    } catch (e) {
      taskRef.current = null;
      setBusy(false);
      message.error(String(e));
    }
  };

  const doTxExport = async () => {
    if (!txDir.trim()) {
      message.warning("请输入导出译文的路径");
      return;
    }
    setTxBusy(true);
    try {
      const r = await api.exportTranslations(txDir.trim());
      setTxResult(r);
      message.success(`译文导出完成：${r.tus} 条`);
    } catch (e) {
      message.error(String(e));
    } finally {
      setTxBusy(false);
    }
  };

  const percent =
    progress && progress.total > 0
      ? Math.round((progress.done / progress.total) * 100)
      : 0;

  return (
    <Space
      direction="vertical"
      size="large"
      style={{ width: "100%", maxWidth: 800 }}
    >
      {/* 译文导出：确定性 汉化 .txt + 译文.csv。 */}
      <Card title="译文导出">
        <Space direction="vertical" style={{ width: "100%" }}>
          <Space.Compact style={{ width: "100%" }}>
            <Input
              addonBefore="目标目录"
              placeholder="/path/to/output"
              value={txDir}
              onChange={(e) => setTxDir(e.target.value)}
              onPressEnter={doTxExport}
            />
            <Button
              onClick={async () => {
                const p = await pickDirectory({ title: "选择译文导出目录" });
                if (p) setTxDir(p);
              }}
            >
              选择…
            </Button>
          </Space.Compact>
          <Button type="primary" loading={txBusy} onClick={doTxExport}>
            导出译文
          </Button>
          {txResult && (
            <Alert
              type="success"
              showIcon
              message={`导出完成：${txResult.tus} 条`}
              description={
                <div
                  style={{
                    fontFamily: "monospace",
                    fontSize: 12,
                    wordBreak: "break-all",
                  }}
                >
                  <div>汉化：{txResult.txt_path}</div>
                  <div>CSV：{txResult.csv_path}</div>
                </div>
              }
            />
          )}
          <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>
            只导出已通过（译文非空）的条目；同名文件会被覆盖。
          </Typography.Paragraph>
        </Space>
      </Card>

      {/* 项目归档：单个压缩包 + 内嵌 SHA-256，携带项目小词库。 */}
      <Card title="导出项目">
        <Space direction="vertical" style={{ width: "100%" }}>
          <Space.Compact style={{ width: "100%" }}>
            <Input
              addonBefore="目标"
              placeholder="/path/to/my-book.zip"
              value={dest}
              onChange={(e) => setDest(e.target.value)}
              onPressEnter={doExport}
            />
            <Button
              onClick={async () => {
                const p = await pickSavePath({
                  title: "导出项目归档",
                  defaultPath: "my-book.felinproj.zip",
                  filters: [{ name: "项目归档", extensions: ["zip"] }],
                });
                if (p) setDest(p);
              }}
            >
              选择…
            </Button>
          </Space.Compact>
          <Button type="primary" loading={busy} onClick={doExport}>
            导出当前项目
          </Button>
          {busy && (
            <Progress
              percent={percent}
              status={busy ? "active" : "normal"}
              format={() =>
                progress ? `${progress.done}/${progress.total}` : "准备中…"
              }
            />
          )}
          {result && (
            <Alert
              type="success"
              showIcon
              message="导出完成"
              description={
                <div
                  style={{
                    fontFamily: "monospace",
                    fontSize: 12,
                    wordBreak: "break-all",
                  }}
                >
                  <div>文件：{result.archive}</div>
                  <div>
                    大小：{result.bytes} 字节，共 {result.files} 个文件
                  </div>
                  <div>SHA-256：{result.sha256}</div>
                </div>
              }
            />
          )}
          <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>
            只导出项目数据（含项目小词库），不含源文件与全局大词库；SHA-256
            校验和已打包在压缩包内部。
          </Typography.Paragraph>
        </Space>
      </Card>
    </Space>
  );
}
