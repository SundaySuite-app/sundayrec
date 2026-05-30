-- F1.3 — per-recording free-text note.
--
-- Adds an optional note column to the recording history so a user can annotate
-- an entry (e.g. "kun preken", "dårlig lyd siste 5 min") from the History panel.
-- Ports the Electron build's per-entry note (capped at 4096 chars in Rust).
-- sqlx::migrate is additive: this runs cleanly on both a fresh and an existing
-- database, leaving older rows with a NULL note.

ALTER TABLE recording ADD COLUMN note TEXT;
