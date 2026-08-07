-- Per-project database — schema v6.
-- Extracted name candidates gain LLM/user-editable category tags (人名、地名…).
-- Stored as a JSON array like every other tag column; the first tag doubles as
-- the entry's category when it is confirmed into a glossary.

ALTER TABLE extracted_names ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';
