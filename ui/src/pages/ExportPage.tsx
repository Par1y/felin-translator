import { useState } from "react";
import { App as AntdApp, Alert, Button, Card, Input, Space, Typography } from "antd";
import type { ExportResult, TranslationExport } from "../types";
import { api } from "../api";

export default function ExportPage() {
  const { message } = AntdApp.useApp();
  const [dest, setDest] = useState("");
  const [result, setResult] = useState<ExportResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [txDir, setTxDir] = useState("");
  const [txResult, setTxResult] = useState<TranslationExport | null>(null);
  const [txBusy, setTxBusy] = useState(false);

  const doExport = async () => {
    if (!dest.trim()) {
      message.warning("请输入导出归档的目标路径");
      return;
    }
    setBusy(true);
    try {
      const r = await api.exportProject(dest.trim());
      setResult(r);
      message.success("项目已导出");
    } catch (e) {
      message.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const doTxExport = async () => {
    if (!txDir.trim()) {
      message.warning("请输入译文导出的目标目录");
      return;
    }
    setTxBusy(true);
    try {
      const r = await api.exportTranslations(txDir.trim());
      setTxResult(r);
      message.success(`译文导出完成：${r.tus} 个 TU`);
    } catch (e) {
      message.error(String(e));
    } finally {
      setTxBusy(false);
    }
  };

  return (
    <Space direction="vertical" size="large" style={{ width: "100%", maxWidth: 800 }}>
      {/* 译文导出：确定性 汉化 .txt + 译文.csv。 */}
      <Card title="译文导出">
        <Space direction="vertical" style={{ width: "100%" }}>
          <Input
            addonBefore="目标目录"
            placeholder="/path/to/output"
            value={txDir}
            onChange={(e) => setTxDir(e.target.value)}
            onPressEnter={doTxExport}
          />
          <Button type="primary" loading={txBusy} onClick={doTxExport}>
            导出译文
          </Button>
          {txResult && (
            <Alert
              type="success"
              showIcon
              message={`导出完成：${txResult.tus} 个 TU`}
              description={
                <div style={{ fontFamily: "monospace", fontSize: 12, wordBreak: "break-all" }}>
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

      {/* 项目归档：单个压缩包 + SHA-256，携带项目小词库。 */}
      <Card title="导出项目">
        <Space direction="vertical" style={{ width: "100%" }}>
          <Input
            addonBefore="目标"
            placeholder="/path/to/my-book.zip"
            value={dest}
            onChange={(e) => setDest(e.target.value)}
            onPressEnter={doExport}
          />
          <Button type="primary" loading={busy} onClick={doExport}>
            导出当前项目
          </Button>
          {result && (
            <Alert
              type="success"
              showIcon
              message="导出完成"
              description={
                <div style={{ fontFamily: "monospace", fontSize: 12, wordBreak: "break-all" }}>
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
            只导出项目数据（含项目小词库），不含源文件与全局大词库。
          </Typography.Paragraph>
        </Space>
      </Card>
    </Space>
  );
}
