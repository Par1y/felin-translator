import { useCallback, useEffect, useState } from "react";
import { Layout, Menu, Tag, Typography } from "antd";
import type { ProjectSummary } from "./types";
import { api } from "./api";
import ProjectsPage from "./pages/ProjectsPage";
import ImportPage from "./pages/ImportPage";
import ReviewPage from "./pages/ReviewPage";
import NamesPage from "./pages/NamesPage";
import ExportPage from "./pages/ExportPage";
import SettingsPage from "./pages/SettingsPage";

const { Sider, Content, Header } = Layout;

type PageKey = "projects" | "import" | "review" | "names" | "export" | "settings";

export default function App() {
  const [page, setPage] = useState<PageKey>("projects");
  const [project, setProject] = useState<ProjectSummary | null>(null);

  const refreshProject = useCallback(async () => {
    try {
      setProject(await api.currentProject());
    } catch {
      setProject(null);
    }
  }, []);

  useEffect(() => {
    void refreshProject();
  }, [refreshProject]);

  const items = [
    { key: "projects", label: "项目" },
    { key: "import", label: "导入", disabled: !project },
    { key: "review", label: "校对", disabled: !project },
    { key: "names", label: "专名", disabled: !project },
    { key: "export", label: "导出", disabled: !project },
    { key: "settings", label: "设置" },
  ];

  return (
    <Layout style={{ height: "100vh" }}>
      <Sider theme="light" width={200} style={{ borderInlineEnd: "1px solid #f0f0f0" }}>
        <div style={{ padding: 16, fontWeight: 600, fontSize: 16 }}>Felin Translator</div>
        <Menu
          mode="inline"
          selectedKeys={[page]}
          items={items}
          onClick={(e) => setPage(e.key as PageKey)}
        />
      </Sider>
      <Layout>
        <Header
          style={{ background: "#fff", display: "flex", alignItems: "center", gap: 8, paddingInline: 16, borderBottom: "1px solid #f0f0f0" }}
        >
          <Typography.Text type="secondary">当前项目：</Typography.Text>
          {project ? <Tag color="blue">{project.name}</Tag> : <Tag>未打开</Tag>}
        </Header>
        <Content style={{ padding: 16, overflow: "auto" }}>
          {page === "projects" && <ProjectsPage project={project} onChange={refreshProject} />}
          {page === "import" && <ImportPage />}
          {page === "review" && <ReviewPage />}
          {page === "names" && <NamesPage />}
          {page === "export" && <ExportPage />}
          {page === "settings" && <SettingsPage />}
        </Content>
      </Layout>
    </Layout>
  );
}
