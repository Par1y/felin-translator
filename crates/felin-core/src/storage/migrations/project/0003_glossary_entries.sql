-- Per-project database — schema v3.
-- Project small glossary: the curated per-project word list that drives
-- translation prompt injection (the plan's 全局大词库 + 项目小词库 model — the
-- big glossary is the shared global pool, this is the project's own small one).
-- Entries are self-contained (japanese/chinese/english/category/tags/aliases are
-- snapshotted) so a project archive carries its own glossary; `name_global_id`
-- records provenance in the global big glossary (cross-file reference, NOT a
-- foreign key — SQLite cannot enforce cross-file FKs).

CREATE TABLE glossary_entries (
    id             INTEGER PRIMARY KEY,
    name_global_id INTEGER,                    -- provenance: global names.id
    japanese       TEXT NOT NULL,
    chinese        TEXT,
    english        TEXT,
    category       TEXT,
    tags           TEXT NOT NULL DEFAULT '[]', -- JSON array of tags
    enabled        INTEGER NOT NULL DEFAULT 1,
    aliases        TEXT NOT NULL DEFAULT '[]', -- JSON array of japanese alias forms
    notes          TEXT,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL,
    UNIQUE(japanese)
);

CREATE INDEX idx_glossary_entries_enabled ON glossary_entries(enabled);
