import { useCallback, useEffect, useRef, useState } from "react";
import {
  App as AntdApp,
  Button,
  Card,
  Checkbox,
  Empty,
  Input,
  InputNumber,
  List,
  Modal,
  Popconfirm,
  Select,
  Space,
  Tabs,
  Tag,
  Tooltip,
  Typography,
} from "antd";
import type { Chapter, TuWithTranslation } from "../types";
import { api } from "../api";
import { useTasks } from "../tasks";

/// TU state → friendly 中文 label + tag color for the proofreading cards.
function tuStatusView(s: string): { text: string; color: string } {
  const map: Record<string, [string, string]> = {
    pending: ["尚未翻译", "blue"],
    queued: ["尚未翻译", "blue"],
    translating: ["翻译中", "processing"],
    translated: ["已译待校", "cyan"],
    reviewing: ["校对中", "orange"],
    approved: ["已通过", "green"],
    exported: ["已导出", "green"],
    interrupted: ["已中断", "default"],
    failed_retryable: ["翻译失败", "red"],
    failed_permanent: ["翻译失败", "red"],
  };
  const [text, color] = map[s] ?? [s, "default"];
  return { text, color };
}

/// Status-group → TU wire states (for the filter tabs).
const STATUS_GROUPS: Record<string, string[]> = {
  all: [],
  todo: ["pending", "queued"],
  translating: ["translating"],
  translated: ["translated", "reviewing", "approved", "exported"],
  failed: ["interrupted", "failed_retryable", "failed_permanent"],
};

/// One 「原文-译文」 proofreading card. Local edits are committed on blur; until
/// the user edits (dirty), the card follows the server's latest value so live
/// pipeline refreshes show up without clobbering in-progress typing.
///
/// One 勾选 (checkbox) drives the selection set (`selected`); the batch bar
/// above the list decides whether the selected TUs are 重译 or 删除. The 专名
/// the TU's source matched are rendered as colored tags (one per name), not a
/// plain text line.
function TuCard({
  tu,
  selected,
  onToggleSelected,
  onChanged,
}: {
  tu: TuWithTranslation;
  selected: boolean;
  onToggleSelected: (id: number, checked: boolean) => void;
  onChanged: () => void;
}) {
  const { message } = AntdApp.useApp();
  const [source, setSource] = useState(tu.source);
  const [trans, setTrans] = useState(tu.final_text ?? "");
  const [dirty, setDirty] = useState(false);

  useEffect(() => {
    if (!dirty) {
      setSource(tu.source);
      setTrans(tu.final_text ?? "");
    }
  }, [tu.source, tu.final_text, dirty]);

  const commitSource = async () => {
    const val = source.trimEnd();
    if (val === tu.source) return;
    try {
      await api.setTuSource(tu.id, val);
      setDirty(false);
      message.success("原文已保存");
    } catch (e) {
      message.error(String(e));
    }
  };

  const commitTrans = async () => {
    if (trans === (tu.final_text ?? "")) return;
    try {
      const demoted = await api.setTranslationText(tu.id, trans);
      setDirty(false);
      if (demoted) message.info("译文已保存（该条已回到校对状态）");
      else message.success("译文已保存");
      onChanged();
    } catch (e) {
      message.error(String(e));
    }
  };

  const approve = async () => {
    try {
      await api.approveTu(tu.id);
      message.success("已通过");
      onChanged();
    } catch (e) {
      message.error(String(e));
    }
  };

  const status = tuStatusView(tu.status);

  /// The enabled small-glossary entries this TU's source hit — what prompt
  /// injection applied — rendered as 专名 tags under the 原文.
  const matched = tu.matched_names ?? [];

  return (
    <Card
      size="small"
      title={
        <Space wrap>
          <Tag color={status.color}>{status.text}</Tag>
          <Typography.Text type="secondary">#{tu.ord + 1}</Typography.Text>
          {tu.translation_status && tu.translation_status !== "draft" && (
            <Typography.Text type="secondary">
              译态：{tu.translation_status}
            </Typography.Text>
          )}
          <Checkbox
            checked={selected}
            onChange={(e) => onToggleSelected(tu.id, e.target.checked)}
          >
            勾选
          </Checkbox>
        </Space>
      }
      styles={{ body: { paddingTop: 8 } }}
    >
      {tu.error && (
        <Typography.Paragraph type="danger" style={{ marginBottom: 8 }}>
          翻译失败：{tu.error}，可在上方批量「重译所选」或「删除所选」
        </Typography.Paragraph>
      )}
      <Space direction="vertical" style={{ width: "100%" }} size="small">
        <div>
          <Typography.Text
            type="secondary"
            style={{ display: "block", marginBottom: 4 }}
          >
            原文
          </Typography.Text>
          <Input.TextArea
            value={source}
            onChange={(e) => {
              setSource(e.target.value);
              setDirty(true);
            }}
            onBlur={commitSource}
            autoSize={{ minRows: 2, maxRows: 8 }}
          />
          {matched.length > 0 ? (
            <div style={{ marginTop: 6 }}>
              <Typography.Text
                type="secondary"
                style={{ fontSize: 12, marginRight: 6 }}
              >
                专名：
              </Typography.Text>
              {matched.map((m, i) => (
                <Tag
                  key={`${m.japanese}-${i}`}
                  color="geekblue"
                  style={{ marginRight: 4, marginBottom: 2 }}
                >
                  {m.chinese && m.chinese.trim()
                    ? `${m.japanese} → ${m.chinese}`
                    : m.japanese}
                </Tag>
              ))}
            </div>
          ) : (
            <Typography.Text
              type="secondary"
              style={{ display: "block", marginTop: 4, fontSize: 12 }}
            >
              专名：—
            </Typography.Text>
          )}
        </div>
        <div>
          <Typography.Text
            type="secondary"
            style={{ display: "block", marginBottom: 4 }}
          >
            译文
          </Typography.Text>
          <Input.TextArea
            value={trans}
            onChange={(e) => {
              setTrans(e.target.value);
              setDirty(true);
            }}
            onBlur={commitTrans}
            autoSize={{ minRows: 2, maxRows: 10 }}
          />
        </div>
        <Space>
          <Button
            size="small"
            type="primary"
            disabled={tu.status === "approved"}
            onClick={approve}
          >
            通过
          </Button>
        </Space>
      </Space>
    </Card>
  );
}

export default function ReviewPage() {
  const { message } = AntdApp.useApp();
  const { translation, translationStart, translationSync, translationStop } = useTasks();
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [chapterId, setChapterId] = useState<number | null>(null);
  const [segmenting, setSegmenting] = useState(false);
  const [blockSize, setBlockSize] = useState(3000);
  const [group, setGroup] = useState<string>("all");

  // Translation section state.
  const [starting, setStarting] = useState(false);
  const [tuRows, setTuRows] = useState<TuWithTranslation[]>([]);
  /// Batch-selection set — the shared checkbox drives both 重译所选 and 删除所选
  /// (both buttons live in the 翻译 bar above).
  const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
  const [retranslateModal, setRetranslateModal] = useState(false);
  const [retranslateInstr, setRetranslateInstr] = useState("");
  const chapterIdRef = useRef<number | null>(null);

  useEffect(() => {
    chapterIdRef.current = chapterId;
  }, [chapterId]);

  const loadChapters = useCallback(
    async (selectFirst = false) => {
      try {
        const chs = await api.listChapters();
        setChapters(chs);
        setChapterId((cur) =>
          selectFirst || cur == null ? (chs[0]?.id ?? null) : cur,
        );
      } catch (e) {
        message.error(String(e));
      }
    },
    [message],
  );

  useEffect(() => {
    void loadChapters();
  }, [loadChapters]);

  const loadTus = useCallback(async (cid: number) => {
    try {
      setTuRows(await api.listTusWithTranslations(cid));
    } catch {
      setTuRows([]);
    }
  }, []);

  useEffect(() => {
    setSelectedIds(new Set());
    if (chapterId == null) {
      setTuRows([]);
      return;
    }
    void loadTus(chapterId);
  }, [chapterId, loadTus]);

  /// Refresh the live translation view (running flag, per-status counts,
  /// activation window) and the selected chapter's TU list.
  const refreshTranslation = useCallback(async () => {
    try {
      translationSync(await api.translationStatus());
    } catch {
      // no project open (page reachable right after a close) — leave as-is
    }
    const cid = chapterIdRef.current;
    if (cid != null) {
      await loadTus(cid);
    }
  }, [translationSync, loadTus]);

  // Pipeline events arrive in the global store; this page re-syncs status and
  // reloads the current chapter's TU rows whenever the store's revision bumps
  // (also runs once on mount to reconcile a run started on another page).
  useEffect(() => {
    void refreshTranslation();
  }, [translation.revision, refreshTranslation]);

  const segment = async () => {
    setSegmenting(true);
    try {
      const r = await api.segmentProject(blockSize);
      message.success(`已分段：${r.chapters} 章，${r.tus} 条`);
      await loadChapters(true);
    } catch (e) {
      message.error(String(e));
    } finally {
      setSegmenting(false);
    }
  };

  const start = async () => {
    setStarting(true);
    try {
      const tid = await api.startTranslation();
      translationStart(tid);
      void refreshTranslation();
    } catch (e) {
      message.error(String(e));
    } finally {
      setStarting(false);
    }
  };

  const retryAll = async () => {
    try {
      const n = await api.retryTranslation("all", []);
      message.success(`已重新入队 ${n} 条`);
      void refreshTranslation();
    } catch (e) {
      message.error(String(e));
    }
  };

  const toggleSelected = (id: number, checked: boolean) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (checked) next.add(id);
      else next.delete(id);
      return next;
    });
  };

  /// The modal's 重译 applies to everything selected regardless of the batch
  /// action toggle (the toggle only picks between 重译所选 and 删除所选 in the
  /// batch bar).
  const confirmRetranslate = async () => {
    const ids = [...selectedIds];
    try {
      const n = await api.retranslateTus(ids, retranslateInstr || undefined);
      message.success(`已重新入队 ${n} 条`);
      setSelectedIds(new Set());
      setRetranslateInstr("");
      setRetranslateModal(false);
      void refreshTranslation();
    } catch (e) {
      message.error(String(e));
    }
  };

  const confirmDelete = async () => {
    const ids = [...selectedIds];
    if (ids.length === 0) return;
    try {
      const n = await api.deleteTus(ids);
      message.success(`已删除 ${n} 个 TU（连同其段落）`);
      setSelectedIds(new Set());
      void refreshTranslation();
    } catch (e) {
      message.error(String(e));
    }
  };

  const groupStates = STATUS_GROUPS[group] ?? [];
  const shown =
    groupStates.length === 0
      ? tuRows
      : tuRows.filter((t) => groupStates.includes(t.status));
  const totalCount = translation.counts.reduce((a, c) => a + c.count, 0);
  // Batch-selection state against the currently-shown TU cards.
  const selectedShown = shown.filter((t) => selectedIds.has(t.id)).length;
  const allShownSelected = shown.length > 0 && selectedShown === shown.length;

  return (
    <Space
      direction="vertical"
      size="large"
      style={{ width: "100%", maxWidth: 900 }}
    >
      {/* ① 分段：块大小 + 自动分段 + 章节选择 + 状态过滤。 */}
      <Card title="分段与选择">
        <Space wrap>
          <Tooltip title="每块目标字符数">
            <InputNumber
              min={200}
              step={500}
              value={blockSize}
              onChange={(v) => setBlockSize(v ?? 3000)}
              addonAfter="字/块"
              style={{ width: 140 }}
            />
          </Tooltip>
          <Button loading={segmenting} onClick={segment}>
            自动分段
          </Button>
          <Select
            style={{ width: 280 }}
            placeholder="选择章节"
            value={chapterId ?? undefined}
            onChange={setChapterId}
            options={chapters.map((c) => ({ value: c.id, label: c.title }))}
          />
          <Tabs
            activeKey={group}
            onChange={setGroup}
            items={[
              { key: "all", label: "全部" },
              { key: "todo", label: "待译" },
              { key: "translating", label: "翻译中" },
              { key: "translated", label: "已译" },
              { key: "failed", label: "失败" },
            ]}
            size="small"
          />
        </Space>
      </Card>

      {/* ② 翻译区：全部翻译 + 停止 + 进度/状态计数。 */}
      <Card
        title="翻译"
        extra={
          <Space>
            <Button
              type="primary"
              loading={starting}
              disabled={translation.running}
              onClick={start}
            >
              全部翻译
            </Button>
            <Button danger disabled={!translation.running} onClick={translationStop}>
              停止
            </Button>
            <Button disabled={translation.running} onClick={retryAll}>
              重试失败项
            </Button>
            <Button
              disabled={selectedIds.size === 0}
              onClick={() => setRetranslateModal(true)}
            >
              重译所选（{selectedIds.size}）
            </Button>
            <Popconfirm
              title={`删除所选 ${selectedIds.size} 个段落？`}
              description="将连同其原文段落一并删除，不可撤销。"
              okText="删除"
              cancelText="取消"
              okButtonProps={{ danger: true }}
              onConfirm={() => void confirmDelete()}
              disabled={selectedIds.size === 0}
            >
              <Button danger disabled={selectedIds.size === 0}>
                删除所选（{selectedIds.size}）
              </Button>
            </Popconfirm>
          </Space>
        }
      >
        <Space wrap>
          {translation.running ? <Tag color="processing">运行中</Tag> : <Tag>未运行</Tag>}
          {translation.activeChapters.length > 0 && (
            <Tag>激活章：{translation.activeChapters.join(", ")}</Tag>
          )}
          {totalCount > 0 && (
            <Typography.Text type="secondary">
              共 {totalCount} 条
            </Typography.Text>
          )}
          {translation.counts.map((c) => (
            <Tag key={c.status} color={tuStatusView(c.status).color}>
              {tuStatusView(c.status).text}: {c.count}
            </Tag>
          ))}
        </Space>
      </Card>

      {/* ③ 批量选择：一个勾选集，操作按钮「重译所选 / 删除所选」都在上方翻译栏。 */}
      <Space wrap style={{ marginBottom: 8 }}>
        <Checkbox
          checked={allShownSelected}
          indeterminate={selectedShown > 0 && !allShownSelected}
          onChange={(e) =>
            setSelectedIds(
              e.target.checked ? new Set(shown.map((t) => t.id)) : new Set(),
            )
          }
        >
          全选（当前筛选，{selectedShown}/{shown.length}）
        </Checkbox>
      </Space>
      {shown.length === 0 ? (
        <Empty description="本章暂无 TU（请先自动分段，或切换筛选）" />
      ) : (
        <List
          grid={{ gutter: 16, column: 1 }}
          dataSource={shown}
          renderItem={(tu) => (
            <List.Item key={tu.id}>
              <TuCard
                tu={tu}
                selected={selectedIds.has(tu.id)}
                onToggleSelected={toggleSelected}
                onChanged={() => void refreshTranslation()}
              />
            </List.Item>
          )}
        />
      )}

      {/* 重译确认子菜单：可填额外指示（适用于当前所有勾选的 TU）。 */}
      <Modal
        title={`重译所选 ${selectedIds.size} 条`}
        open={retranslateModal}
        onOk={() => void confirmRetranslate()}
        onCancel={() => setRetranslateModal(false)}
        okText="重译"
        cancelText="取消"
      >
        <Input.TextArea
          placeholder="可选：本次重译的额外指示（如“注意敬语”）"
          value={retranslateInstr}
          onChange={(e) => setRetranslateInstr(e.target.value)}
          autoSize={{ minRows: 3, maxRows: 6 }}
        />
      </Modal>
    </Space>
  );
}
