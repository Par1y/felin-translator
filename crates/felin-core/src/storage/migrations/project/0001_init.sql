-- Per-project database — schema v1.
-- Note: cross-database references to the global glossary (matched_name_id,
-- name_global_id) are plain INTEGERs; SQLite cannot enforce cross-file foreign
-- keys, so they are intentionally NOT declared REFERENCES.

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT
);

CREATE TABLE chapters (
    id     INTEGER PRIMARY KEY,
    title  TEXT NOT NULL,
    ord    INTEGER NOT NULL,          -- sort key (plan's "order"; reserved word avoided)
    status TEXT NOT NULL DEFAULT 'pending'
);

CREATE TABLE paragraphs (
    id          TEXT PRIMARY KEY,     -- UUID; stable minimal unit
    chapter_id  INTEGER NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
    ord         INTEGER NOT NULL,
    text        TEXT NOT NULL,
    page_num    INTEGER,              -- starting page for cross-page-merged paragraphs
    page_score  REAL,                 -- NULL when evaluator disabled / no score
    -- ok | low_score | suspect | page_failed_recovered
    ocr_status  TEXT NOT NULL DEFAULT 'ok',
    ocr_meta    TEXT,                 -- JSON: quality_warning / best_score / fallback / source ptr
    source_file TEXT
);

CREATE TABLE tus (
    id            INTEGER PRIMARY KEY,
    chapter_id    INTEGER NOT NULL REFERENCES chapters(id) ON DELETE CASCADE,
    paragraph_ids TEXT NOT NULL,      -- JSON array of paragraph UUIDs
    ord           INTEGER NOT NULL,
    budget        INTEGER,
    status        TEXT NOT NULL DEFAULT 'pending'
);

CREATE TABLE translations (
    id          INTEGER PRIMARY KEY,
    tu_id       INTEGER NOT NULL REFERENCES tus(id) ON DELETE CASCADE,
    status      TEXT NOT NULL DEFAULT 'draft',
    source_hash TEXT,                 -- normalized-source hash for translation memory
    llm_text    TEXT,                 -- raw model output, preserved forever
    final_text  TEXT,                 -- editable, human-approved draft
    instruction TEXT,                 -- per-item manual guidance injected on re-translate
    attempts    TEXT,                 -- JSON array of prior attempts
    error       TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE extracted_names (
    id                INTEGER PRIMARY KEY,
    japanese          TEXT NOT NULL,
    matched_name_id   INTEGER,        -- cross-DB ref to global names.id (not enforced)
    candidate_chinese TEXT,
    -- new | matched | confirmed | rejected
    status            TEXT NOT NULL DEFAULT 'new',
    segments          TEXT,           -- JSON
    notes             TEXT
);

CREATE TABLE project_name_overrides (
    id               INTEGER PRIMARY KEY,
    name_global_id   INTEGER NOT NULL, -- cross-DB ref to global names.id (not enforced)
    enabled          INTEGER NOT NULL DEFAULT 1,
    override_chinese TEXT,
    note             TEXT,
    UNIQUE(name_global_id)
);

CREATE TABLE exports (
    id         INTEGER PRIMARY KEY,
    path       TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_paragraphs_chapter_ord ON paragraphs(chapter_id, ord);
CREATE INDEX idx_paragraphs_source_page ON paragraphs(source_file, page_num);
CREATE INDEX idx_tus_chapter_ord        ON tus(chapter_id, ord);
CREATE INDEX idx_tus_status             ON tus(status);
CREATE INDEX idx_translations_tu        ON translations(tu_id);
CREATE INDEX idx_translations_srchash   ON translations(source_hash);
CREATE INDEX idx_extracted_names_status ON extracted_names(status);
