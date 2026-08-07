import { useCallback, useEffect, useState } from "react";
import { App as AntdApp, Button, Card, Empty, Input, List, Modal, Popconfirm, Space, Tag, Typography } from "antd";
import type { ProjectSummary } from "../types";
import { api } from "../api";
import { pickFile } from "../dialog";

export default function ProjectsPage({
  project,
  onChange,
}: {
  project: ProjectSummary | null;
  onChange: () => Promise<void>;
}) {
  const { message } = AntdApp.useApp();
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [name, setName] = useState("");
  const [loading, setLoading] = useState(false);
  const [importPath, setImportPath] = useState("");
  const [importing, setImporting] = useState(false);
  const [renaming, setRenaming] = useState<ProjectSummary | null>(null);
  const [renameValue, setRenameValue] = useState("");

  const refresh = useCallback(async () => {
    try {
      setProjects(await api.listProjects());
    } catch (e) {
      message.error(String(e));
    }
  }, [message]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const create = async () => {
    if (!name.trim()) {
      message.warning("请输入项目名称");
      return;
    }
    setLoading(true);
    try {
      await api.createProject(name.trim());
      setName("");
      await onChange();
      await refresh();
      message.success("已创建并打开项目");
    } catch (e) {
      message.error(String(e));
    } finally {
      setLoading(false);
    }
  };

  const open = async (slug: string) => {
    try {
      await api.openProject(slug);
      await onChange();
      message.success("已打开项目");
    } catch (e) {
      message.error(String(e));
    }
  };

  const close = async () => {
    try {
      await api.closeProject();
      await onChange();
      message.success("已关闭项目");
    } catch (e) {
      message.error(String(e));
    }
  };

  const openRename = (p: ProjectSummary) => {
    setRenameValue(p.name);
    setRenaming(p);
  };

  const confirmRename = async () => {
    if (!renaming) return;
    const next = renameValue.trim();
    if (!next) {
      message.warning("项目名称不能为空");
      return;
    }
    try {
      await api.renameProject(renaming.slug, next);
      setRenaming(null);
      // The open project's display name changed → sync the main title (App
      // re-pulls current_project via onChange).
      await onChange();
      await refresh();
      message.success("已重命名项目");
    } catch (e) {
      message.error(String(e));
    }
  };

  const remove = async (slug: string) => {
    try {
      await api.deleteProject(slug);
      // Deleting the open project closes it in the backend → App returns to the
      // un-opened state.
      if (project?.slug === slug) {
        await onChange();
      }
      await refresh();
      message.success("项目已删除");
    } catch (e) {
      message.error(String(e));
    }
  };

  const importArchive = async () => {
    if (!importPath.trim()) {
      message.warning("请输入项目归档路径（.felinproj）");
      return;
    }
    setImporting(true);
    try {
      const s = await api.importProject(importPath.trim());
      setImportPath("");
      await refresh();
      message.success(`已导入项目：${s.name}`);
    } catch (e) {
      message.error(String(e));
    } finally {
      setImporting(false);
    }
  };

  return (
    <Space direction="vertical" size="large" style={{ width: "100%", maxWidth: 800 }}>
      <Card title="新建项目">
        <Space.Compact style={{ width: "100%" }}>
          <Input
            placeholder="项目名称（例：少女民俗学）"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onPressEnter={create}
          />
          <Button type="primary" loading={loading} onClick={create}>
            创建
          </Button>
        </Space.Compact>
      </Card>

      <Card title="项目列表" extra={project ? <Button onClick={close}>关闭当前</Button> : null}>
        {projects.length === 0 ? (
          <Empty description="还没有项目" />
        ) : (
          <List
            dataSource={projects}
            renderItem={(p) => (
              <List.Item
                actions={[
                  <Button
                    key="open"
                    type="link"
                    disabled={project?.slug === p.slug}
                    onClick={() => open(p.slug)}
                  >
                    {project?.slug === p.slug ? "已打开" : "打开"}
                  </Button>,
                  <Button key="rename" type="link" onClick={() => openRename(p)}>
                    重命名
                  </Button>,
                  <Popconfirm
                    key="delete"
                    title={`删除项目「${p.name}」？`}
                    description="将删除其全部数据，此操作不可撤销，归档不会被删除。"
                    okText="删除"
                    cancelText="取消"
                    okButtonProps={{ danger: true }}
                    onConfirm={() => remove(p.slug)}
                  >
                    <Button type="link" danger>
                      删除
                    </Button>
                  </Popconfirm>,
                ]}
              >
                <List.Item.Meta
                  title={
                    <Space>
                      {p.name}
                      {project?.slug === p.slug && <Tag color="blue">当前</Tag>}
                    </Space>
                  }
                  description={
                    <Typography.Text type="secondary">
                      {p.slug} · {p.created_at}
                    </Typography.Text>
                  }
                />
              </List.Item>
            )}
          />
        )}
      </Card>

      {/* Import a project archive (zip; SHA-256 verified on import). */}
      <Card title="导入项目">
        <Space.Compact style={{ width: "100%" }}>
          <Input
            placeholder="/path/to/my-book.zip"
            value={importPath}
            onChange={(e) => setImportPath(e.target.value)}
            onPressEnter={importArchive}
          />
          <Button
            onClick={async () => {
              const p = await pickFile({
                title: "选择项目归档",
                filters: [{ name: "项目归档", extensions: ["zip"] }],
              });
              if (p) setImportPath(p);
            }}
          >
            选择…
          </Button>
          <Button loading={importing} onClick={importArchive}>
            导入归档
          </Button>
        </Space.Compact>
      </Card>

      {/* Rename a project's display name (disk dir / slug unchanged). */}
      <Modal
        title="重命名项目"
        open={renaming !== null}
        onOk={confirmRename}
        onCancel={() => setRenaming(null)}
        okText="保存"
        cancelText="取消"
      >
        <Input
          value={renameValue}
          onChange={(e) => setRenameValue(e.target.value)}
          onPressEnter={confirmRename}
          placeholder="项目显示名称"
          autoFocus
        />
      </Modal>
    </Space>
  );
}
