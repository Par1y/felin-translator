-- Per-project database — schema v4.
-- TU source override: the user may edit a TU's 原文 (source) in the review UI.
-- The edited text is stored here; the underlying paragraphs stay untouched so
-- re-segmentation isn't affected, and tu_source() returns the override when
-- present (the pipeline reads the same function).

ALTER TABLE tus ADD COLUMN source_override TEXT;
