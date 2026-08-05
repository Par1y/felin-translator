import { useState } from "react";
import { App as AntdApp, Alert, Button, Card, Input, Space, Typography } from "antd";
import type { ExportResult } from "../types";
import { api } from "../api";

export default function ExportPage() {
  const { message } = AntdApp.useApp();
  const [dest, setDest] = useState("");
  const [result, setResult] = useState<ExportResult | null>(null);
  const [busy, setBusy] = useState(false);

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

  return (
    <Space direction="vertical" size="large" style={{ width: "100%", maxWidth: 800 }}>
      {/* Single zstd-compressed zip + a sibling .sha256; excludes source files
          and the global glossary. Translated-text export comes in a later step. */}
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
            只导出项目数据，不含源文件与全局词库。
          </Typography.Paragraph>
        </Space>
      </Card>
    </Space>
  );
}
