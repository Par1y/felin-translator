-- Per-project database — schema v5.
-- 专名别名功能彻底移除（用户指示）：删除 glossary_entries 的 aliases 列。
-- 别名历史数据删除为接受行为；其余列原样保留（self-contained 快照语义不变）。
-- SQLite（尤其旧版本）无可靠的标准 DROP COLUMN，故重建表：建新表 → 复制选定列 →
-- 删旧表 → 改名，索引随之重建。迁移整体由 runner 包裹在单事务内执行。

CREATE TABLE glossary_entries_new (
    id             INTEGER PRIMARY KEY,
    name_global_id INTEGER,
    japanese       TEXT NOT NULL,
    chinese        TEXT,
    english        TEXT,
    category       TEXT,
    tags           TEXT NOT NULL DEFAULT '[]', -- JSON array of tags
    enabled        INTEGER NOT NULL DEFAULT 1,
    notes          TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL,
    UNIQUE(japanese)
);

INSERT INTO glossary_entries_new
    (id, name_global_id, japanese, chinese, english, category, tags, enabled, notes, created_at, updated_at)
SELECT id, name_global_id, japanese, chinese, english, category, tags, enabled, notes, created_at, updated_at
FROM glossary_entries;

DROP TABLE glossary_entries;
ALTER TABLE glossary_entries_new RENAME TO glossary_entries;

CREATE INDEX idx_glossary_entries_enabled ON glossary_entries(enabled);
