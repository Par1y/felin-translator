import { useCallback, useEffect, useState } from "react";
import { App as AntdApp, Button, Card, Empty, Input, List, Space, Tag, Typography } from "antd";
import type { ProjectSummary } from "../types";
import { api } from "../api";

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
          <Button loading={importing} onClick={importArchive}>
            导入归档
          </Button>
        </Space.Compact>
      </Card>
    </Space>
  );
}
