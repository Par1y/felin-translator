import { useEffect, useState } from "react";
import { App as AntdApp, Card, Descriptions, Tag } from "antd";
import type { AppInfo } from "../types";
import { api } from "../api";

export default function SettingsPage() {
  const { message } = AntdApp.useApp();
  const [info, setInfo] = useState<AppInfo | null>(null);

  useEffect(() => {
    api.appInfo().then(setInfo).catch((e) => message.error(String(e)));
  }, [message]);

  return (
    // Full settings (LLM/OCR providers, budgets, concurrency) come in a later step.
    <Card title="诊断" style={{ maxWidth: 800 }}>
      {info && (
        <Descriptions column={1} bordered size="small">
          <Descriptions.Item label="版本">{info.version}</Descriptions.Item>
          <Descriptions.Item label="数据目录">{info.data_dir}</Descriptions.Item>
          <Descriptions.Item label="OCR 引擎">{info.sidecar}</Descriptions.Item>
          <Descriptions.Item label="OCR 引擎状态">
            {info.sidecar_present ? <Tag color="green">已就绪</Tag> : <Tag color="red">未找到</Tag>}
          </Descriptions.Item>
          <Descriptions.Item label="全局词库条目">{info.glossary_names}</Descriptions.Item>
        </Descriptions>
      )}
    </Card>
  );
}
