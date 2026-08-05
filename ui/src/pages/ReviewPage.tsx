import { useCallback, useEffect, useState } from "react";
import {
  App as AntdApp,
  Button,
  Card,
  Empty,
  InputNumber,
  Select,
  Space,
  Table,
  Tag,
  Tooltip,
  Typography,
  type TableProps,
} from "antd";
import type { Chapter, Paragraph } from "../types";
import { api } from "../api";

export default function ReviewPage() {
  const { message } = AntdApp.useApp();
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [chapterId, setChapterId] = useState<number | null>(null);
  const [paras, setParas] = useState<Paragraph[]>([]);
  const [tuCount, setTuCount] = useState<number | null>(null);
  const [segmenting, setSegmenting] = useState(false);
  const [blockSize, setBlockSize] = useState(3000);

  const loadChapters = useCallback(
    async (selectFirst = false) => {
      try {
        const chs = await api.listChapters();
        setChapters(chs);
        setChapterId((cur) => (selectFirst || cur == null ? (chs[0]?.id ?? null) : cur));
      } catch (e) {
        message.error(String(e));
      }
    },
    [message],
  );

  useEffect(() => {
    void loadChapters();
  }, [loadChapters]);

  useEffect(() => {
    if (chapterId == null) {
      setParas([]);
      setTuCount(null);
      return;
    }
    api.listParagraphs(chapterId).then(setParas).catch((e) => message.error(String(e)));
    api.listTus(chapterId).then((t) => setTuCount(t.length)).catch(() => setTuCount(null));
  }, [chapterId, message]);

  const segment = async () => {
    setSegmenting(true);
    try {
      const r = await api.segmentProject(blockSize);
      message.success(`已分段：${r.chapters} 章，${r.tus} 个 TU`);
      await loadChapters(true);
    } catch (e) {
      message.error(String(e));
    } finally {
      setSegmenting(false);
    }
  };

  const columns: TableProps<Paragraph>["columns"] = [
    { title: "#", dataIndex: "ord", width: 60 },
    { title: "页", dataIndex: "page_num", width: 70, render: (v: number | null) => v ?? "—" },
    {
      title: "评分",
      dataIndex: "page_score",
      width: 80,
      render: (v: number | null) => (v == null ? "—" : v.toFixed(2)),
    },
    {
      title: "状态",
      dataIndex: "ocr_status",
      width: 150,
      render: (s: string) => (
        <Tag color={s === "ok" ? "green" : s === "low_score" ? "orange" : "default"}>{s}</Tag>
      ),
    },
    {
      title: "原文",
      dataIndex: "text",
      render: (t: string) => <span style={{ whiteSpace: "pre-wrap" }}>{t}</span>,
    },
  ];

  return (
    <Space direction="vertical" size="large" style={{ width: "100%" }}>
      <Card
        title="校对"
        extra={
          <Space>
            {/* Soft target block size: paragraphs are grouped into ~equal blocks. */}
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
              style={{ width: 260 }}
              placeholder="选择章节"
              value={chapterId ?? undefined}
              onChange={setChapterId}
              options={chapters.map((c) => ({ value: c.id, label: `${c.title}（${c.status}）` }))}
            />
          </Space>
        }
      >
        {chapters.length === 0 ? (
          <Empty description="尚无章节，请先导入并分段" />
        ) : (
          <>
            <Typography.Paragraph type="secondary">
              本章 {paras.length} 段落{tuCount == null ? "" : `，${tuCount} 个 TU`}
            </Typography.Paragraph>
            <Table rowKey="id" size="small" columns={columns} dataSource={paras} pagination={{ pageSize: 20 }} />
          </>
        )}
      </Card>
    </Space>
  );
}
