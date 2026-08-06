-- Global glossary database — schema v2.
-- Entries gain tag-based organization and per-entry enable/disable:
--   tags    — JSON array of user / LLM / source-generated tags
--             (人名, 地名, 所属数据库, 来源项目…), used for quick
--             enable/disable/search in the 专名 UI.
--   enabled — per-entry toggle; translation prompt injection only considers
--             enabled entries reachable from an enabled project small-glossary
--             (see project v3 glossary_entries).

ALTER TABLE names ADD COLUMN tags   TEXT NOT NULL DEFAULT '[]';
ALTER TABLE names ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;
