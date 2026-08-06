import { useCallback, useEffect, useState } from "react";
import {
  App as AntdApp,
  Button,
  Card,
  Input,
  List,
  Modal,
  Select,
  Space,
  Switch,
  Table,
  Tag,
  Tabs,
  Typography,
  type TableProps,
} from "antd";
import type { GlossaryEntry, GlossaryName } from "../types";
import { api } from "../api";

function tagsOf(tags?: string[]) {
  return (tags ?? []).map((t) => <Tag key={t}>{t}</Tag>);
}

export default function NamesPage() {
  const { message } = AntdApp.useApp();
  const [tab, setTab] = useState("project");

  // ---- 项目小词库 ------------------------------------------------------------
  const [entries, setEntries] = useState<GlossaryEntry[]>([]);
  const [q, setQ] = useState("");
  const [selectedKeys, setSelectedKeys] = useState<React.Key[]>([]);
  const [editOpen, setEditOpen] = useState(false);
  const [editId, setEditId] = useState<number | null>(null);
  const [editForm, setEditForm] = useState({
    japanese: "",
    chinese: "",
    english: "",
    category: "",
    tags: [] as string[],
    aliases: [] as string[],
    notes: "",
  });

  // ---- 全局大词库 ------------------------------------------------------------
  const [globalNames, setGlobalNames] = useState<GlossaryName[]>([]);
  const [gq, setGq] = useState("");
  const [fromGlobalOpen, setFromGlobalOpen] = useState(false);
  const [globalResults, setGlobalResults] = useState<GlossaryName[]>([]);
  const [tagEdit, setTagEdit] = useState<GlossaryName | null>(null);
  const [tagDraft, setTagDraft] = useState<string[]>([]);

  const loadEntries = useCallback(
    async (query?: string) => {
      try {
        setEntries(await api.listGlossaryEntries(query));
      } catch (e) {
        message.error(String(e));
      }
    },
    [message],
  );

  const loadGlobal = useCallback(
    async (query?: string) => {
      try {
        setGlobalNames(await api.listGlossary(query || undefined, 200));
      } catch (e) {
        message.error(String(e));
      }
    },
    [message],
  );

  useEffect(() => {
    void loadEntries();
    void loadGlobal();
  }, [loadEntries, loadGlobal]);

  const searchEntries = () => void loadEntries(q.trim() || undefined);
  const searchGlobal = () => void loadGlobal(gq.trim() || undefined);

  const openEdit = (e?: GlossaryEntry) => {
    if (!e) {
      setEditId(null);
      setEditForm({ japanese: "", chinese: "", english: "", category: "", tags: [], aliases: [], notes: "" });
    } else {
      setEditId(e.id);
      setEditForm({
        japanese: e.japanese,
        chinese: e.chinese ?? "",
        english: e.english ?? "",
        category: e.category ?? "",
        tags: e.tags,
        aliases: e.aliases,
        notes: e.notes ?? "",
      });
    }
    setEditOpen(true);
  };

  const saveEntry = async () => {
    if (!editForm.japanese.trim()) {
      message.warning("日文不能为空");
      return;
    }
    const input = {
      japanese: editForm.japanese.trim(),
      chinese: editForm.chinese.trim() || null,
      english: editForm.english.trim() || null,
      category: editForm.category.trim() || null,
      tags: editForm.tags,
      aliases: editForm.aliases,
      notes: editForm.notes.trim() || null,
    };
    try {
      if (editId == null) await api.addGlossaryEntry({ ...input, name_global_id: null });
      else await api.updateGlossaryEntry(editId, input);
      message.success("已保存");
      setEditOpen(false);
      await loadEntries(q.trim() || undefined);
    } catch (e) {
      message.error(String(e));
    }
  };

  const toggleEnabled = async (id: number, enabled: boolean) => {
    try {
      await api.setEntryEnabled(id, enabled);
      await loadEntries(q.trim() || undefined);
    } catch (e) {
      message.error(String(e));
    }
  };

  const batchSetEnabled = async (enabled: boolean) => {
    if (selectedKeys.length === 0) {
      message.warning("请先勾选词条");
      return;
    }
    try {
      for (const id of selectedKeys) await api.setEntryEnabled(Number(id), enabled);
      message.success(`已${enabled ? "启用" : "禁用"} ${selectedKeys.length} 条`);
      setSelectedKeys([]);
      await loadEntries(q.trim() || undefined);
    } catch (e) {
      message.error(String(e));
    }
  };

  const deleteEntry = async (id: number) => {
    try {
      await api.deleteGlossaryEntry(id);
      message.success("已删除");
      await loadEntries(q.trim() || undefined);
    } catch (e) {
      message.error(String(e));
    }
  };

  const searchFromGlobal = async (query: string) => {
    try {
      setGlobalResults(await api.listGlossary(query || undefined, 100));
    } catch (e) {
      message.error(String(e));
    }
  };

  const addFromGlobal = async (g: GlossaryName) => {
    try {
      await api.addGlossaryEntry({
        name_global_id: g.id,
        japanese: g.japanese,
        chinese: g.chinese,
        english: g.english,
        category: g.category,
        tags: g.tags,
        aliases: [],
        notes: g.notes,
      });
      message.success(`已添加 ${g.japanese}`);
      await loadEntries(q.trim() || undefined);
    } catch (e) {
      message.error(String(e));
    }
  };

  const toggleGlobalEnabled = async (id: number, enabled: boolean) => {
    try {
      await api.setGlobalNameEnabled(id, enabled);
      await loadGlobal(gq.trim() || undefined);
    } catch (e) {
      message.error(String(e));
    }
  };

  const saveGlobalTags = async () => {
    if (!tagEdit) return;
    try {
      await api.setGlobalNameTags(tagEdit.id, tagDraft);
      message.success("标签已保存");
      setTagEdit(null);
      await loadGlobal(gq.trim() || undefined);
    } catch (e) {
      message.error(String(e));
    }
  };

  const entryColumns: TableProps<GlossaryEntry>["columns"] = [
    { title: "日文", dataIndex: "japanese", width: 200 },
    { title: "中文", dataIndex: "chinese", width: 160, render: (v: string | null) => v ?? "—" },
    { title: "标签", dataIndex: "tags", render: (v: string[]) => tagsOf(v) },
    { title: "别名", dataIndex: "aliases", render: (v: string[]) => tagsOf(v) },
    {
      title: "启用",
      dataIndex: "enabled",
      width: 70,
      render: (v: boolean, r) => (
        <Switch size="small" checked={v} onChange={(c) => toggleEnabled(r.id, c)} />
      ),
    },
    {
      title: "操作",
      width: 130,
      render: (_: unknown, r) => (
        <Space size={0}>
          <Button size="small" type="link" onClick={() => openEdit(r)}>
            编辑
          </Button>
          <Button size="small" type="link" danger onClick={() => deleteEntry(r.id)}>
            删除
          </Button>
        </Space>
      ),
    },
  ];

  const globalColumns: TableProps<GlossaryName>["columns"] = [
    { title: "日文", dataIndex: "japanese", width: 200 },
    { title: "中文", dataIndex: "chinese", width: 160, render: (v: string | null) => v ?? "—" },
    { title: "来源", dataIndex: "source", width: 160, render: (v: string | null) => v ?? "—" },
    {
      title: "标签",
      dataIndex: "tags",
      render: (v: string[], r) => (
        <Button size="small" type="link" onClick={() => { setTagEdit(r); setTagDraft(v); }}>
          {v.length > 0 ? tagsOf(v) : "添加标签"}
        </Button>
      ),
    },
    {
      title: "启用",
      dataIndex: "enabled",
      width: 70,
      render: (v: boolean, r) => (
        <Switch size="small" checked={v} onChange={(c) => toggleGlobalEnabled(r.id, c)} />
      ),
    },
  ];

  return (
    <Space direction="vertical" size="large" style={{ width: "100%", maxWidth: 1000 }}>
      <Tabs
        activeKey={tab}
        onChange={setTab}
        items={[
          {
            key: "project",
            label: "项目小词库",
            children: (
              <Card
                title="项目小词库"
                extra={
                  <Space wrap>
                    <Input.Search
                      placeholder="搜索日文/中文/标签/别名"
                      value={q}
                      onChange={(e) => setQ(e.target.value)}
                      onSearch={searchEntries}
                      allowClear
                      style={{ width: 220 }}
                    />
                    <Button onClick={() => openEdit()}>新建</Button>
                    <Button onClick={() => setFromGlobalOpen(true)}>从全局搜索添加</Button>
                    <Button onClick={() => batchSetEnabled(true)}>批量启用</Button>
                    <Button onClick={() => batchSetEnabled(false)}>批量禁用</Button>
                  </Space>
                }
              >
                <Table
                  rowKey="id"
                  size="small"
                  columns={entryColumns}
                  dataSource={entries}
                  rowSelection={{ selectedRowKeys: selectedKeys, onChange: setSelectedKeys }}
                  pagination={{ pageSize: 10 }}
                  locale={{ emptyText: "暂无词条" }}
                />
                <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>
                  翻译 prompt 只注入本词库中「启用」的词条；禁用后不再注入。
                </Typography.Paragraph>
              </Card>
            ),
          },
          {
            key: "global",
            label: "全局大词库",
            children: (
              <Card
                title="全局大词库"
                extra={
                  <Input.Search
                    placeholder="搜索日文/中文/标签"
                    value={gq}
                    onChange={(e) => setGq(e.target.value)}
                    onSearch={searchGlobal}
                    allowClear
                    style={{ width: 220 }}
                  />
                }
              >
                <Table
                  rowKey="id"
                  size="small"
                  columns={globalColumns}
                  dataSource={globalNames}
                  pagination={{ pageSize: 10 }}
                  locale={{ emptyText: "暂无词条" }}
                />
                <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>
                  全局共享词库不直接注入翻译 prompt；需要时从全局「添加」到项目小词库。
                </Typography.Paragraph>
              </Card>
            ),
          },
        ]}
      />

      {/* 词条编辑/新建 */}
      <Modal
        title={editId == null ? "新建词条" : "编辑词条"}
        open={editOpen}
        onOk={() => void saveEntry()}
        onCancel={() => setEditOpen(false)}
        okText="保存"
        cancelText="取消"
      >
        <Space direction="vertical" style={{ width: "100%" }}>
          <Input addonBefore="日文" value={editForm.japanese} onChange={(e) => setEditForm({ ...editForm, japanese: e.target.value })} />
          <Input addonBefore="中文" value={editForm.chinese} onChange={(e) => setEditForm({ ...editForm, chinese: e.target.value })} />
          <Input addonBefore="英文" value={editForm.english} onChange={(e) => setEditForm({ ...editForm, english: e.target.value })} />
          <Input addonBefore="类别" value={editForm.category} onChange={(e) => setEditForm({ ...editForm, category: e.target.value })} />
          <Select
            mode="tags"
            placeholder="标签（回车添加）"
            value={editForm.tags}
            onChange={(v: string[]) => setEditForm({ ...editForm, tags: v })}
            style={{ width: "100%" }}
          />
          <Select
            mode="tags"
            placeholder="别名（回车添加）"
            value={editForm.aliases}
            onChange={(v: string[]) => setEditForm({ ...editForm, aliases: v })}
            style={{ width: "100%" }}
          />
          <Typography.Text>备注</Typography.Text>
          <Input.TextArea value={editForm.notes} onChange={(e) => setEditForm({ ...editForm, notes: e.target.value })} />
        </Space>
      </Modal>

      {/* 从全局搜索添加 */}
      <Modal
        title="从全局搜索添加"
        open={fromGlobalOpen}
        onCancel={() => setFromGlobalOpen(false)}
        footer={null}
      >
        <Space direction="vertical" style={{ width: "100%" }}>
          <Input.Search
            placeholder="搜索全局词库"
            onSearch={searchFromGlobal}
          />
          <List
            size="small"
            dataSource={globalResults}
            locale={{ emptyText: "输入关键词搜索" }}
            renderItem={(g) => (
              <List.Item
                actions={[
                  <Button size="small" type="primary" onClick={() => addFromGlobal(g)}>
                    添加
                  </Button>,
                ]}
              >
                <Space wrap>
                  <Typography.Text>{g.japanese}</Typography.Text>
                  <Typography.Text type="secondary">→ {g.chinese ?? "—"}</Typography.Text>
                  {tagsOf(g.tags)}
                </Space>
              </List.Item>
            )}
          />
        </Space>
      </Modal>

      {/* 全局词条标签编辑 */}
      <Modal
        title="编辑标签"
        open={tagEdit != null}
        onOk={() => void saveGlobalTags()}
        onCancel={() => setTagEdit(null)}
        okText="保存"
        cancelText="取消"
      >
        <Select
          mode="tags"
          placeholder="标签（回车添加）"
          value={tagDraft}
          onChange={(v: string[]) => setTagDraft(v)}
          style={{ width: "100%" }}
        />
      </Modal>
    </Space>
  );
}
