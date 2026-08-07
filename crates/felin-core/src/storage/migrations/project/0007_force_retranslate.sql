-- Per-project database — schema v7.
-- Explicit re-translate (重译所选) must actually call the LLM even when an
-- identical source was already approved elsewhere (translation-memory dedup
-- would otherwise short-circuit and silently reuse the old translation).
-- `force_retranslate` is set to 1 by retranslate_tus/tu and cleared by the
-- pipeline worker when it reads it, so exactly one run performs a fresh call.

ALTER TABLE tus ADD COLUMN force_retranslate INTEGER NOT NULL DEFAULT 0;
