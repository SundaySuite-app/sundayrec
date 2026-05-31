-- SundayRec migration 0004 — wake-failure / test-wake history (Fase 5.2 parity)
--
-- Durable backing store for the wake-failure log, replacing the Electron build's
-- `wakeFailureHistory` electron-store array. Records two kinds of outcome:
--   - a *missed* wake (a scheduled recording never ran), and
--   - a manual *test-wake* result (`test_ok` / `test_fail`).
-- The renderer's "wake-diagnostikk" panel reads this newest-first and offers a
-- clear-history action. Capped at 20 rows in the store layer (the Electron
-- WAKE_FAILURE_MAX), trimming oldest. Timestamps are unix ms (REAL) to match the
-- recording table's convention.
create table if not exists wake_failure (
  id           TEXT PRIMARY KEY,
  ts           REAL NOT NULL,           -- unix ms when the outcome was recorded
  scheduled_at TEXT NOT NULL,           -- ISO string the wake was meant to fire
  kind         TEXT NOT NULL            -- 'missed' | 'test_ok' | 'test_fail'
                 CHECK (kind IN ('missed','test_ok','test_fail')),
  label        TEXT NOT NULL,
  reason       TEXT,                    -- free-form ('no_resume','on_battery', …)
  delta_sec    INTEGER                  -- expected↔observed delta (test-wake only)
);

-- The panel shows newest-first.
create index if not exists idx_wake_failure_ts on wake_failure (ts DESC);
