-- Global glossary database — schema v1.
-- The single source of truth for proper nouns across all projects.

CREATE TABLE names (
    id         INTEGER PRIMARY KEY,
    japanese   TEXT NOT NULL,
    english    TEXT,
    chinese    TEXT,
    category   TEXT,
    notes      TEXT,
    source     TEXT,
    -- imported | draft | confirmed
    status     TEXT NOT NULL DEFAULT 'draft',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(japanese)
);

CREATE TABLE name_aliases (
    id            INTEGER PRIMARY KEY,
    name_id       INTEGER NOT NULL REFERENCES names(id) ON DELETE CASCADE,
    japanese_form TEXT NOT NULL,
    UNIQUE(japanese_form)
);

CREATE TABLE name_history (
    id         INTEGER PRIMARY KEY,
    name_id    INTEGER NOT NULL REFERENCES names(id) ON DELETE CASCADE,
    field      TEXT NOT NULL,
    old        TEXT,
    new        TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_name_aliases_form ON name_aliases(japanese_form);
CREATE INDEX idx_name_history_name ON name_history(name_id);
