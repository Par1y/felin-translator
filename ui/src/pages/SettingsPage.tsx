import { useCallback, useEffect, useState } from "react";
import {
  App as AntdApp,
  Alert,
  Button,
  Card,
  Descriptions,
  Divider,
  Input,
  InputNumber,
  Space,
  Switch,
  Tag,
  Tooltip,
  Typography,
} from "antd";
import type {
  AppInfo,
  OcrConfig,
  OcrProviderConfig,
  OcrSettings,
  PromptConfig,
  TranslationSettings,
} from "../types";
import { api } from "../api";

const { Text } = Typography;

// Mirrors felin_core::pipeline::default_guidelines() — the project 总则 default.
const DEFAULT_GUIDELINES = [
  "你是日译中翻译校对助手。请把日文原文翻译成简体中文。",
  "规则：",
  "- 保持原文排版与空行结构，段落对应关系不变。",
  "- 对话与引用格式保持一致。",
  "- 称呼与敬称按中文习惯处理；专名必须使用词表译名。",
  "- 只输出译文本身，不要任何解释、注释或额外内容。",
].join("\n");

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

  // 提示词（Prompt）：项目总则 + felin.toml [prompt] 三模板
  const [guidelines, setGuidelinesText] = useState("");
  const [guidelinesError, setGuidelinesError] = useState<string | null>(null);
  const [prompt, setPrompt] = useState<PromptConfig | null>(null);
  const [promptError, setPromptError] = useState<string | null>(null);
  const [promptSaving, setPromptSaving] = useState(false);

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
    api
      .getGuidelines()
      .then((g) => {
        setGuidelinesText(g);
        setGuidelinesError(null);
      })
      .catch(() => setGuidelinesError("未打开项目"));
    api
      .getPromptConfig()
      .then((p) => {
        setPrompt(p);
        setPromptError(null);
      })
      .catch((e) => setPromptError(String(e)));
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

  // --- 提示词（Prompt） ---

  const saveGuidelines = async () => {
    try {
      await api.setGuidelines(guidelines);
      message.success("总则已保存");
    } catch (e) {
      message.error(`保存总则失败：${e}`);
    }
  };

  const restoreGuidelines = async () => {
    setGuidelinesText(DEFAULT_GUIDELINES);
    try {
      await api.setGuidelines(DEFAULT_GUIDELINES);
      message.success("已恢复默认总则");
    } catch (e) {
      message.error(`恢复默认总则失败：${e}`);
    }
  };

  const savePrompt = async () => {
    if (!prompt) return;
    setPromptSaving(true);
    try {
      await api.setPromptConfig(prompt);
      setPromptError(null);
      message.success("已写入 felin.toml（注释/排版保持）");
    } catch (e) {
      message.error(`保存提示词失败：${e}`);
    } finally {
      setPromptSaving(false);
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
              <Space direction="vertical" size="middle" style={{ width: "100%" }}>
                {orderedProviders.map((p, i) => (
                  <Card
                    key={p.name}
                    size="small"
                    style={{ width: "100%" }}
                    title={
                      <Space size={8}>
                        <Tag color="blue">{i + 1}</Tag>
                        <Text strong>{p.name}</Text>
                      </Space>
                    }
                    extra={
                      <Space size={4}>
                        <Tooltip title="提前（调用顺序序号减小）">
                          <Button
                            size="small"
                            disabled={i === 0}
                            onClick={() => move(i, -1)}
                            aria-label="提前"
                          >
                            ↑
                          </Button>
                        </Tooltip>
                        <Tooltip title="延后（调用顺序序号增大）">
                          <Button
                            size="small"
                            disabled={i === orderedProviders.length - 1}
                            onClick={() => move(i, 1)}
                            aria-label="延后"
                          >
                            ↓
                          </Button>
                        </Tooltip>
                        <Tooltip title="是否参与调用（禁用会从调用顺序中排除）">
                          <Switch
                            size="small"
                            checked={p.enabled}
                            onChange={(v) => patchProvider(p.name, { enabled: v })}
                            checkedChildren="启用"
                            unCheckedChildren="停用"
                          />
                        </Tooltip>
                      </Space>
                    }
                  >
                    <Space direction="vertical" style={{ width: "100%" }}>
                      <Input
                        addonBefore={p.name === "browser_sse" ? "base_url" : "接口"}
                        value={p.endpoint}
                        onChange={(e) => patchProvider(p.name, { endpoint: e.target.value })}
                        placeholder={
                          p.name === "browser_sse" ? "http://localhost:9222" : "https://host/v1"
                        }
                      />
                      {p.name === "llm_vision" && (
                        <Input
                          addonBefore="模型"
                          value={p.model}
                          onChange={(e) => patchProvider(p.name, { model: e.target.value })}
                          placeholder="step-3.7-flash"
                        />
                      )}
                      {p.name !== "browser_sse" && (
                        <Input.Password
                          addonBefore="密钥"
                          value={p.api_key}
                          onChange={(e) => patchProvider(p.name, { api_key: e.target.value })}
                          placeholder="sk-... 或 ${ENV}"
                        />
                      )}
                    </Space>
                  </Card>
                ))}
              </Space>

              <Card
                size="small"
                title="评估阶段"
                extra={
                  <Tooltip title="是否启用 OCR 质量评估">
                    <Switch
                      size="small"
                      checked={ocrCfg.evaluator.enabled}
                      onChange={(v) => patchEvaluator({ enabled: v })}
                      checkedChildren="启用"
                      unCheckedChildren="停用"
                    />
                  </Tooltip>
                }
              >
                <Space direction="vertical" style={{ width: "100%" }}>
                  <Input
                    addonBefore="接口"
                    value={ocrCfg.evaluator.endpoint}
                    onChange={(e) => patchEvaluator({ endpoint: e.target.value })}
                    placeholder="https://host/v1"
                  />
                  <Input
                    addonBefore="模型"
                    value={ocrCfg.evaluator.model}
                    onChange={(e) => patchEvaluator({ model: e.target.value })}
                    placeholder="step-3.7-flash"
                  />
                  <Input.Password
                    addonBefore="密钥"
                    value={ocrCfg.evaluator.api_key}
                    onChange={(e) => patchEvaluator({ api_key: e.target.value })}
                    placeholder="sk-... 或 ${ENV}"
                  />
                </Space>
              </Card>

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

      <Card title="提示词（Prompt）">
        <Space direction="vertical" size="middle" style={{ width: "100%" }}>
          <Text type="secondary">
            翻译总则（项目级，存于项目数据库）—— 作为 {"{guidelines}"} 占位符注入翻译 system 模板
          </Text>
          {guidelinesError ? (
            <Alert
              type="warning"
              showIcon
              message={guidelinesError}
              description="翻译总则按项目保存，需要先打开一个项目后再编辑。"
            />
          ) : (
            <Space direction="vertical" style={{ width: "100%" }}>
              <Input.TextArea
                value={guidelines}
                onChange={(e) => setGuidelinesText(e.target.value)}
                rows={4}
                placeholder="项目翻译总则…"
              />
              <Space>
                <Button type="primary" onClick={saveGuidelines}>
                  保存总则
                </Button>
                <Button onClick={restoreGuidelines}>恢复默认</Button>
              </Space>
            </Space>
          )}

          <Divider plain style={{ margin: "8px 0" }}>
            Prompt 模板（felin.toml [prompt]，保存后立即生效）
          </Divider>

          <Alert
            type="info"
            showIcon
            style={{ marginBottom: 4 }}
            message="占位符说明"
            description={
              <Descriptions column={1} size="small" style={{ marginTop: 4 }}>
                <Descriptions.Item label="{guidelines}">
                  项目翻译总则（本卡片上方编辑，存于项目数据库）
                </Descriptions.Item>
                <Descriptions.Item label="{instruction}">
                  该 TU 的人工指示（校对页逐条翻译时填写，重译时带入）
                </Descriptions.Item>
                <Descriptions.Item label="{glossary}">
                  该 TU 命中的小词库专名（「日文 → 中文」，翻译必须使用）
                </Descriptions.Item>
                <Descriptions.Item label="{context}">
                  上一段已批准译文（仅供风格与称谓参考）
                </Descriptions.Item>
                <Descriptions.Item label="{source}">
                  待翻译原文（唯一的必填内容）
                </Descriptions.Item>
              </Descriptions>
            }
          />
          <Alert
            type="warning"
            showIcon
            style={{ marginBottom: 8 }}
            message="按空行分段；整段占位符全部为空时该段不发送"
            description="模板按空行拆成段落逐段渲染：某段引用的占位符（如 {instruction}、{glossary}、{context}）全部为空时，整段不会发给模型；不含占位符的段落原样保留。因此 `{source}` 必须出现在 User 模板中，否则译文将为空。"
          />

          {promptError && (
            <Alert type="error" showIcon message="无法读取提示词配置" description={promptError} />
          )}
          {prompt && (
            <Space direction="vertical" style={{ width: "100%" }}>
              <div>
                <Text strong>专名抽取 Prompt</Text>
                <Text type="secondary">（extract_system）—— 留空 = 不发送 system 消息，仅发送章节原文</Text>
              </div>
              <Input.TextArea
                value={prompt.extract_system}
                onChange={(e) => setPrompt({ ...prompt, extract_system: e.target.value })}
                rows={3}
                placeholder="专名抽取 system 消息，例如“从给定日文文本中抽取专有名词…”"
              />
              <div>
                <Text strong>专名自动打标签 Prompt</Text>
                <Text type="secondary">
                  （extract_tags_system）—— 用于「自动打标签」，应指明可选类别（人名/地名/…）；
                  留空时「自动打标签」按钮会拒绝执行
                </Text>
              </div>
              <Input.TextArea
                value={prompt.extract_tags_system}
                onChange={(e) => setPrompt({ ...prompt, extract_tags_system: e.target.value })}
                rows={3}
                placeholder="专名分类 system 消息，例如“判断每个专有名词的类别（人名/地名/…）…”"
              />
              <div>
                <Text strong>翻译 System 模板</Text>
                <Text type="secondary">
                  （translation_system）—— 占位符：{"{guidelines}"} / {"{instruction}"} /{" "}
                  {"{glossary}"}；留空 = 不发送 system 消息
                </Text>
              </div>
              <Input.TextArea
                value={prompt.translation_system}
                onChange={(e) => setPrompt({ ...prompt, translation_system: e.target.value })}
                rows={4}
                placeholder={`{guidelines}\n\n附加要求（优先级高于总则）：{instruction}\n\n专名参考（词表，必须使用）：{glossary}`}
              />
              <div>
                <Text strong>翻译 User 模板</Text>
                <Text type="secondary">
                  （translation_user）—— 占位符：{"{context}"} / {"{source}"}；留空 = 只发送原文
                </Text>
              </div>
              <Input.TextArea
                value={prompt.translation_user}
                onChange={(e) => setPrompt({ ...prompt, translation_user: e.target.value })}
                rows={4}
                placeholder={`【上文参考（已校对，仅供风格与称谓参考，勿重复翻译）】\n{context}\n\n【待翻译原文】\n{source}`}
              />
              <Space>
                <Button type="primary" loading={promptSaving} onClick={savePrompt}>
                  保存 Prompt
                </Button>
                <Button
                  onClick={() =>
                    api
                      .getPromptConfig()
                      .then((p) => {
                        setPrompt(p);
                        setPromptError(null);
                      })
                      .catch((e) => setPromptError(String(e)))
                  }
                >
                  重新读取
                </Button>
              </Space>
            </Space>
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
