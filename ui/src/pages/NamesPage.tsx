import { useCallback, useEffect, useState } from "react";
import {
  App as AntdApp,
  Button,
  Card,
  Checkbox,
  Input,
  InputNumber,
  Space,
  Table,
  Tag,
  type TableProps,
} from "antd";
import type { ExtractedName, GlossaryName } from "../types";
import { api } from "../api";

export default function NamesPage() {
  const { message } = AntdApp.useApp();
  const [endpoint, setEndpoint] = useState("");
  const [model, setModel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [hasKey, setHasKey] = useState(false);
  const [candidates, setCandidates] = useState<ExtractedName[]>([]);
  const [extracting, setExtracting] = useState(false);
  const [glossary, setGlossary] = useState<GlossaryName[]>([]);
  const [csvPath, setCsvPath] = useState("");
  const [jp, setJp] = useState(0);
  const [zh, setZh] = useState(1);
  const [en, setEn] = useState<number | null>(2);
  const [al, setAl] = useState<number | null>(3);
  const [hasHeader, setHasHeader] = useState(true);

  const loadConfig = useCallback(async () => {
    try {
      const c = await api.getLlmConfig();
      setEndpoint(c.endpoint);
      setModel(c.model);
      setHasKey(c.has_key);
    } catch (e) {
      message.error(String(e));
    }
  }, [message]);

  const loadCandidates = useCallback(async () => {
    try {
      setCandidates(await api.listExtracted("new"));
    } catch (e) {
      message.error(String(e));
    }
  }, [message]);

  const loadGlossary = useCallback(async () => {
    try {
      setGlossary(await api.listGlossary(200));
    } catch (e) {
      message.error(String(e));
    }
  }, [message]);

  useEffect(() => {
    void loadConfig();
    void loadCandidates();
    void loadGlossary();
  }, [loadConfig, loadCandidates, loadGlossary]);

  const saveConfig = async () => {
    try {
      await api.setLlmConfig(endpoint, model, apiKey || undefined);
      setApiKey("");
      await loadConfig();
      message.success("已保存");
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

  const confirm = async (id: number) => {
    try {
      await api.confirmExtracted(id);
      await Promise.all([loadCandidates(), loadGlossary()]);
    } catch (e) {
      message.error(String(e));
    }
  };

  const reject = async (id: number) => {
    try {
      await api.rejectExtracted(id);
      await loadCandidates();
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
      const n = await api.importGlossaryCsv(csvPath.trim(), {
        japanese: jp,
        chinese: zh,
        english: en ?? undefined,
        aliases: al ?? undefined,
        has_header: hasHeader,
      });
      message.success(`导入 ${n} 条`);
      setCsvPath("");
      await loadGlossary();
    } catch (e) {
      message.error(String(e));
    }
  };

  // PLACEHOLDER_NAMES_JSX

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
            if (val !== (v ?? "")) api.updateExtracted(r.id, val).catch((err) => message.error(String(err)));
          }}
        />
      ),
    },
    {
      title: "操作",
      width: 150,
      render: (_: unknown, r) => (
        <Space>
          <Button size="small" type="link" onClick={() => confirm(r.id)}>
            通过
          </Button>
          <Button size="small" type="link" danger onClick={() => reject(r.id)}>
            拒绝
          </Button>
        </Space>
      ),
    },
  ];

  const glossaryColumns: TableProps<GlossaryName>["columns"] = [
    { title: "日文", dataIndex: "japanese" },
    { title: "中文", dataIndex: "chinese", render: (v: string | null) => v ?? "—" },
    { title: "状态", dataIndex: "status", width: 100, render: (s: string) => <Tag>{s}</Tag> },
    { title: "来源", dataIndex: "source", render: (v: string | null) => v ?? "—" },
  ];

  return (
    <Space direction="vertical" size="large" style={{ width: "100%", maxWidth: 1000 }}>
      <Card title="翻译模型">
        <Space direction="vertical" style={{ width: "100%" }}>
          <Input
            addonBefore="接口"
            value={endpoint}
            onChange={(e) => setEndpoint(e.target.value)}
            placeholder="https://api.stepfun.com/v1"
          />
          <Input
            addonBefore="模型"
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="step-3.7-flash"
          />
          <Input.Password
            addonBefore="密钥"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder={hasKey ? "已保存（留空不修改）" : "sk-..."}
          />
          <Button type="primary" onClick={saveConfig}>
            保存
          </Button>
        </Space>
      </Card>

      <Card
        title="专名抽取"
        extra={
          <Button type="primary" loading={extracting} onClick={runExtract}>
            运行抽取
          </Button>
        }
      >
        <Table
          rowKey="id"
          size="small"
          columns={candidateColumns}
          dataSource={candidates}
          pagination={{ pageSize: 10 }}
          locale={{ emptyText: "暂无候选" }}
        />
      </Card>

      <Card title="导入词库（CSV）">
        <Space direction="vertical" style={{ width: "100%" }}>
          <Input
            addonBefore="路径"
            value={csvPath}
            onChange={(e) => setCsvPath(e.target.value)}
            placeholder="/path/to/glossary.csv"
            onPressEnter={importCsv}
          />
          <Space wrap>
            <InputNumber addonBefore="日文列" min={0} value={jp} onChange={(v) => setJp(v ?? 0)} />
            <InputNumber addonBefore="中文列" min={0} value={zh} onChange={(v) => setZh(v ?? 1)} />
            <InputNumber addonBefore="英文列" min={0} value={en ?? undefined} onChange={(v) => setEn(v)} />
            <InputNumber addonBefore="别名列" min={0} value={al ?? undefined} onChange={(v) => setAl(v)} />
            <Checkbox checked={hasHeader} onChange={(e) => setHasHeader(e.target.checked)}>
              含表头
            </Checkbox>
            <Button onClick={importCsv}>导入</Button>
          </Space>
        </Space>
      </Card>

      <Card title="全局词库">
        <Table
          rowKey="id"
          size="small"
          columns={glossaryColumns}
          dataSource={glossary}
          pagination={{ pageSize: 10 }}
        />
      </Card>
    </Space>
  );
}
