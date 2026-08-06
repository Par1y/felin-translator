-- Schema v2: one translation row per TU.
-- The pipeline treats `translations` as keyed by tu_id (upsert on re-translate,
-- attempts JSON holds prior drafts). Enforce that invariant at the DB level.
-- The table is empty at this point, so swapping the index is safe.
DROP INDEX IF EXISTS idx_translations_tu;
CREATE UNIQUE INDEX idx_translations_tu ON translations(tu_id);
