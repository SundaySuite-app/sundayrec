-- SundayRec migration 0005 — the opt-in telemetry outbox (E3.3)
--
-- Durable backing store for the anonymous quality/crash/usage reports the user
-- has explicitly opted in to. Modelled on `0003_upload_queue.sql` — the pure
-- state machine lives in `sundayrec-core::telemetry::queue` and this table only
-- persists the `TelemetryEntry` rows, so a report survives a restart or a long
-- offline stretch. Timestamps are unix ms (INTEGER, UTC), matching the core's
-- i64 fields; `status` stores the same lowercase strings the core serialises.
--
-- What is NOT in a row: the columns hold an id, four numbers, a dedup key, an
-- error string and one `payload_json` blob. That blob is a
-- `sundayrec_core::telemetry::TelemetryPayload`, whose TYPES cannot represent
-- audio, transcripts, names, e-mail, file paths, device names or the church name
-- (see that module's docs). The table inherits the guarantee rather than
-- restating it.
--
-- Existing installs get this table on the next launch and it stays EMPTY:
-- consent defaults to off, nothing is enqueued without it, and revoking purges
-- it.
create table if not exists telemetry_queue (
  id           TEXT PRIMARY KEY,
  created_at   INTEGER NOT NULL,         -- unix ms; the payload's build time
  schema_ver   INTEGER NOT NULL,         -- TELEMETRY_SCHEMA at build time
  dedup_key    TEXT NOT NULL,            -- 'crash:<ts>' | 'quality:<ts>' | 'counters:<ts>'
  payload_json TEXT NOT NULL,            -- the rendered, already-sanitised payload
  attempts     INTEGER NOT NULL DEFAULT 0,
  next_attempt INTEGER NOT NULL,         -- unix ms; earliest the sender may retry
  last_error   TEXT,
  status       TEXT NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending','sending','failed'))
);

-- The drain advances a watermark so a batch is only built once, but two drains
-- racing (a startup drain and the periodic one) would both read the watermark
-- before either wrote it. Enforce the invariant in storage too, exactly as the
-- upload queue does for (service, file_path).
create unique index if not exists idx_telemetry_queue_dedup
  on telemetry_queue (dedup_key);

-- The sender picks the earliest due pending entry.
create index if not exists idx_telemetry_queue_due
  on telemetry_queue (status, next_attempt);

-- The bound (QUEUE_MAX, drop-oldest) sorts by age.
create index if not exists idx_telemetry_queue_age
  on telemetry_queue (created_at);
