import { useCallback, useEffect, useState } from "react";
import {
  App as AntdApp,
  Alert,
  Button,
  Card,
  Descriptions,
  Input,
  InputNumber,
  Space,
  Switch,
  Tag,
  Tooltip,
} from "antd";
import type { AppInfo, OcrConfig, OcrProviderConfig, OcrSettings, TranslationSettings } from "../types";
import { api } from "../api";

export default function SettingsPage() {
  const { message } = AntdApp.useApp();
  const [info, setInfo] = useState<AppInfo | null>(null);

  // 翻译模型
  const [endpoint, setEndpoint] = useState("");
  const [model, setModel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [hasKey, setHasKey] = useState(false);
  const [testing, setTesting] = useState(false);

  // 翻译行为（并发 / 窗口 / 记忆去重 / 停止即中断）
  const [ts, setTs] = useState<TranslationSettings | null>(null);

  // OCR 选项
  const [ocr, setOcr] = useState<OcrSettings | null>(null);

  // OCR 配置（config.yaml 就地编辑）
  const [ocrCfg, setOcrCfg] = useState<OcrConfig | null>(null);
  const [ocrCfgError, setOcrCfgError] = useState<string | null>(null);
  const [ocrCfgSaving, setOcrCfgSaving] = useState(false);

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

  const loadAll = useCallback(() => {
    void loadConfig();
    api.getTranslationSettings().then(setTs).catch(() => setTs(null));
    api.getOcrSettings().then(setOcr).catch(() => setOcr(null));
    api.appInfo().then(setInfo).catch((e) => message.error(String(e)));
    api
      .getOcrConfig()
      .then((c) => {
        setOcrCfg(c);
        setOcrCfgError(null);
      })
      .catch((e) => {
        setOcrCfg(null);
        setOcrCfgError(String(e));
      });
  }, [loadConfig, message]);

  useEffect(() => {
    loadAll();
  }, [loadAll]);

  const saveLlm = async () => {
    try {
      await api.setLlmConfig(endpoint || undefined, model || undefined, apiKey || undefined);
      setApiKey("");
      await loadConfig();
      message.success("翻译模型已保存");
    } catch (e) {
      message.error(String(e));
    }
  };

  const testLlm = async () => {
    setTesting(true);
    try {
      await api.testLlmConnection();
      message.success("连接成功");
    } catch (e) {
      message.error(String(e));
    } finally {
      setTesting(false);
    }
  };

  const saveTs = async (patch: Partial<TranslationSettings>) => {
    if (!ts) return;
    const next = { ...ts, ...patch };
    setTs(next);
    try {
      await api.setTranslationSettings(next);
    } catch (e) {
      message.error(`保存翻译设置失败：${e}`);
      void api.getTranslationSettings().then(setTs).catch(() => setTs(null));
    }
  };

  const saveOcr = async (patch: Partial<OcrSettings>) => {
    if (!ocr) return;
    const next = { ...ocr, ...patch };
    setOcr(next);
    try {
      await api.setOcrSettings(next);
    } catch (e) {
      message.error(`保存 OCR 设置失败：${e}`);
      void api.getOcrSettings().then(setOcr).catch(() => setOcr(null));
    }
  };

  // --- OCR 配置（config.yaml） ---

  const patchProvider = (name: string, patch: Partial<OcrProviderConfig>) => {
    setOcrCfg((c) =>
      c
        ? { ...c, providers: c.providers.map((p) => (p.name === name ? { ...p, ...patch } : p)) }
        : c,
    );
  };

  const patchEvaluator = (patch: Partial<OcrConfig["evaluator"]>) => {
    setOcrCfg((c) => (c ? { ...c, evaluator: { ...c.evaluator, ...patch } } : c));
  };

  // Reorder the call-order list: dir = -1 (earlier) / +1 (later).
  const move = (idx: number, dir: -1 | 1) => {
    setOcrCfg((c) => {
      if (!c) return c;
      const j = idx + dir;
      if (j < 0 || j >= c.order.length) return c;
      const order = [...c.order];
      [order[idx], order[j]] = [order[j], order[idx]];
      return { ...c, order };
    });
  };

  // Providers rendered in call-order sequence (PROVIDER_NAMES if not listed).
  const orderedProviders = ocrCfg
    ? ocrCfg.order
        .map((name) => ocrCfg.providers.find((p) => p.name === name))
        .filter((p): p is OcrProviderConfig => !!p)
    : [];
  if (ocrCfg) {
    for (const p of ocrCfg.providers) {
      if (!orderedProviders.some((q) => q.name === p.name)) orderedProviders.push(p);
    }
  }

  const saveOcrCfg = async () => {
    if (!ocrCfg) return;
    setOcrCfgSaving(true);
    try {
      await api.setOcrConfig(ocrCfg);
      setOcrCfgError(null);
      message.success("已写入 config.yaml（注释/格式可能被规范化）");
    } catch (e) {
      message.error(`保存 OCR 配置失败：${e}`);
    } finally {
      setOcrCfgSaving(false);
    }
  };

  const disabled = !ocrCfg;

  return (
    <Space direction="vertical" size="large" style={{ width: "100%", maxWidth: 800 }}>
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
          <Space>
            <Button type="primary" onClick={saveLlm}>
              保存
            </Button>
            <Button loading={testing} onClick={testLlm}>
              测试连通
            </Button>
          </Space>
        </Space>
      </Card>

      <Card title="翻译行为">
        <Space wrap>
          <Tooltip title="并发翻译线程数（1–8），兼作 LLM 速率上限">
            <InputNumber
              min={1}
              max={8}
              value={ts?.workers ?? 2}
              onChange={(v) => void saveTs({ workers: v ?? 1 })}
              addonBefore="并发 N"
              style={{ width: 120 }}
            />
          </Tooltip>
          <Tooltip title="章节激活窗口（1–5）：最多 W 个章节的 TU 同时在译">
            <InputNumber
              min={1}
              max={5}
              value={ts?.window ?? 1}
              onChange={(v) => void saveTs({ window: v ?? 1 })}
              addonBefore="窗口 W"
              style={{ width: 120 }}
            />
          </Tooltip>
          <Tooltip title="按规范化源文哈希去重：同源已批准译文直接复用，跳过 LLM">
            <Switch
              checked={ts?.memory_dedup ?? true}
              onChange={(v) => void saveTs({ memory_dedup: v })}
              checkedChildren="记忆去重"
              unCheckedChildren="记忆去重"
            />
          </Tooltip>
          <Tooltip title="停止时是否中断在飞 TU（关闭 = 让在飞项完成）">
            <Switch
              checked={ts?.stop_aborts_inflight ?? false}
              onChange={(v) => void saveTs({ stop_aborts_inflight: v })}
              checkedChildren="停止即中断"
              unCheckedChildren="停止即中断"
            />
          </Tooltip>
        </Space>
      </Card>

      <Card title="OCR 选项">
        <Space wrap>
          <Tooltip title="图片目录导入的并发识别线程数">
            <InputNumber
              min={1}
              max={16}
              value={ocr?.batch_workers ?? 4}
              onChange={(v) => void saveOcr({ batch_workers: v ?? 1 })}
              addonBefore="批量工作数"
              style={{ width: 140 }}
            />
          </Tooltip>
          <Tooltip title="扫描图片目录时是否递归子目录">
            <Switch
              checked={ocr?.batch_recursive ?? false}
              onChange={(v) => void saveOcr({ batch_recursive: v })}
              checkedChildren="递归子目录"
              unCheckedChildren="递归子目录"
            />
          </Tooltip>
        </Space>
      </Card>

      <Card title="OCR 配置（config.yaml）">
        <Space direction="vertical" size="middle" style={{ width: "100%" }}>
          {info && (
            <Descriptions column={1} size="small" style={{ maxWidth: 700 }}>
              <Descriptions.Item label="配置文件">
                {info.ocr_config_path ? (
                  <span>
                    {info.ocr_config_path}
                    {info.ocr_config_present ? (
                      <Tag color="green" style={{ marginLeft: 8 }}>
                        存在
                      </Tag>
                    ) : (
                      <Tag color="red" style={{ marginLeft: 8 }}>
                        不存在
                      </Tag>
                    )}
                  </span>
                ) : (
                  "未配置"
                )}
              </Descriptions.Item>
            </Descriptions>
          )}
          <Alert
            type="warning"
            showIcon
            message="就地改写 ocr-router 的 config.yaml"
            description="提供商、调用顺序与评估阶段改动会直接写入该文件；其余段落与 ${ENV} 占位符保留，但文件注释与排版会被规范化。API 密钥原样写回，不会记录日志。"
          />
          {ocrCfgError && (
            <Alert type="error" showIcon message="无法读取 OCR 配置" description={ocrCfgError} />
          )}
          {ocrCfg && (
            <>
              <Space direction="vertical" style={{ width: "100%" }}>
                {orderedProviders.map((p, i) => (
                  <Space key={p.name} wrap align="center" style={{ width: "100%" }}>
                    <Space.Compact>
                      <Tooltip title="提前">
                        <Button
                          size="small"
                          disabled={i === 0}
                          onClick={() => move(i, -1)}
                          aria-label="提前"
                        >
                          ↑
                        </Button>
                      </Tooltip>
                      <Tooltip title="延后">
                        <Button
                          size="small"
                          disabled={i === orderedProviders.length - 1}
                          onClick={() => move(i, 1)}
                          aria-label="延后"
                        >
                          ↓
                        </Button>
                      </Tooltip>
                    </Space.Compact>
                    <Tag>{p.name}</Tag>
                    <Tooltip title="是否参与调用（禁用会从调用顺序中排除）">
                      <Switch
                        size="small"
                        checked={p.enabled}
                        onChange={(v) => patchProvider(p.name, { enabled: v })}
                        checkedChildren="启用"
                        unCheckedChildren="停用"
                      />
                    </Tooltip>
                    <Input
                      addonBefore={p.name === "browser_sse" ? "base_url" : "接口"}
                      style={{ width: 340 }}
                      value={p.endpoint}
                      onChange={(e) => patchProvider(p.name, { endpoint: e.target.value })}
                      placeholder={
                        p.name === "browser_sse"
                          ? "http://localhost:9222"
                          : "https://host/v1"
                      }
                    />
                    {p.name === "llm_vision" && (
                      <Input
                        addonBefore="模型"
                        style={{ width: 200 }}
                        value={p.model}
                        onChange={(e) => patchProvider(p.name, { model: e.target.value })}
                        placeholder="step-3.7-flash"
                      />
                    )}
                    {p.name !== "browser_sse" && (
                      <Input.Password
                        addonBefore="密钥"
                        style={{ width: 260 }}
                        value={p.api_key}
                        onChange={(e) => patchProvider(p.name, { api_key: e.target.value })}
                        placeholder="sk-... 或 ${ENV}"
                      />
                    )}
                  </Space>
                ))}
              </Space>
              <Space direction="vertical" style={{ width: "100%" }}>
                <Space wrap align="center">
                  <Tag>评估阶段</Tag>
                  <Switch
                    size="small"
                    checked={ocrCfg.evaluator.enabled}
                    onChange={(v) => patchEvaluator({ enabled: v })}
                    checkedChildren="启用"
                    unCheckedChildren="停用"
                  />
                  <Input
                    addonBefore="接口"
                    style={{ width: 340 }}
                    value={ocrCfg.evaluator.endpoint}
                    onChange={(e) => patchEvaluator({ endpoint: e.target.value })}
                    placeholder="https://host/v1"
                  />
                  <Input
                    addonBefore="模型"
                    style={{ width: 200 }}
                    value={ocrCfg.evaluator.model}
                    onChange={(e) => patchEvaluator({ model: e.target.value })}
                    placeholder="step-3.7-flash"
                  />
                  <Input.Password
                    addonBefore="密钥"
                    style={{ width: 260 }}
                    value={ocrCfg.evaluator.api_key}
                    onChange={(e) => patchEvaluator({ api_key: e.target.value })}
                    placeholder="sk-... 或 ${ENV}"
                  />
                </Space>
              </Space>
              <Space>
                <Button type="primary" loading={ocrCfgSaving} disabled={disabled} onClick={saveOcrCfg}>
                  保存 OCR 配置
                </Button>
                <Button
                  onClick={() =>
                    api
                      .getOcrConfig()
                      .then((c) => {
                        setOcrCfg(c);
                        setOcrCfgError(null);
                      })
                      .catch((e) => setOcrCfgError(String(e)))
                  }
                >
                  重新读取
                </Button>
              </Space>
            </>
          )}
        </Space>
      </Card>

      <Card title="诊断">
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
    </Space>
  );
}
