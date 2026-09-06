-- SundayRec migration 0006 — the e-mail relay's outbox and its "said it once" ledger
--
-- The relay is the light way to get an e-mail when a recording fails: no SMTP
-- host, no app password, just an address and a confirmation click. The client
-- renders the mail and posts it to the SundaySuite endpoint, which sends it.
-- These two tables are the client's durable half.
--
-- Shaped after `0005_telemetry_queue.sql`, deliberately and almost column for
-- column: the pure state machine lives in `sundayrec_core::relay` and this only
-- persists `RelayEntry` rows, so an alert survives a restart, a lost network or
-- a week offline. Timestamps are unix ms (INTEGER, UTC), matching the core's i64
-- fields; `kind`, `event` and `status` store the same lowercase strings the core
-- serialises.
--
-- ## What a row holds, and why it holds the rendered mail
--
-- `payload_json` is the REQUEST BODY as it will be sent — subject, text and HTML
-- already composed — not the facts they were built from. A row queued by version
-- N is therefore sent unchanged by version N+1: an update that rewords a
-- template cannot silently reword an alert that was already written, and the
-- mail the volunteer receives is the mail the build that saw the failure wrote.
--
-- That does mean a row CAN carry a church name and a person's name, which the
-- telemetry queue's rows structurally cannot. That is the point of the feature —
-- an alert with no idea which church it is about helps nobody — and it is why
-- the relay is a service the user asks for by double opt-in rather than
-- collection. The row stays on this machine until it is delivered, and the
-- endpoint stores no message content at all (see PRIVACY.md).
--
-- Existing installs get both tables on the next launch and they stay EMPTY:
-- nothing is queued until somebody enrols an address and confirms it.
create table if not exists notify_outbox (
  id           TEXT PRIMARY KEY,
  created_at   INTEGER NOT NULL,         -- unix ms; when the row was built
  kind         TEXT NOT NULL             -- what the row DOES at the endpoint
                 CHECK (kind IN ('subscribe','send','unsubscribe')),
  -- Which mail a 'send' row carries: 'failure' | 'missed' | 'receipt' | 'test'.
  -- NULL for the two kinds that carry no mail of their own — a 'subscribe' row
  -- causes the endpoint to send the confirmation, and an 'unsubscribe' row
  -- causes no mail at all. NOT constrained to be non-null for 'send' here: the
  -- CHECK would have to name the same four words a third time (core enum, this
  -- column, the endpoint's KINDS list), and `RelayEntry`'s Option<RelayMessageKind>
  -- is where that invariant is actually enforced.
  event        TEXT
                 CHECK (event IS NULL OR event IN ('failure','missed','receipt','test')),
  dedup_key    TEXT NOT NULL,            -- 'failure:<code>:<ts>' | 'missed:<occurrence>' | …
  payload_json TEXT NOT NULL,            -- the rendered request body
  attempts     INTEGER NOT NULL DEFAULT 0,
  next_attempt INTEGER NOT NULL,         -- unix ms; earliest the sender may try
  last_error   TEXT,
  status       TEXT NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending','sending','failed'))
);

-- Two racing enqueues must not become two e-mails. `check_missed` runs at
-- startup AND after every wake, and a failure can be reported from more than one
-- observer, so the invariant is enforced in storage exactly as the upload queue
-- does for (service, file_path) — `insert_capped` treats the collision as a
-- benign no-op rather than an error.
create unique index if not exists idx_notify_outbox_dedup
  on notify_outbox (dedup_key);

-- The pump picks the earliest due pending row.
create index if not exists idx_notify_outbox_due
  on notify_outbox (status, next_attempt);

-- The bound (RELAY_QUEUE_MAX, drop-oldest) and the freshness sweep both sort by
-- age.
create index if not exists idx_notify_outbox_age
  on notify_outbox (created_at);

-- ── "Have we already said this?" ────────────────────────────────────────────
--
-- The durable half of the once-policy. `crate::email::AlertGate` answers the
-- same question for SMTP and answers it in RAM, which is right for a throttle
-- window and wrong for an occurrence: `check_missed` runs at startup and after
-- every wake, so a Sunday that was missed would produce one e-mail per restart
-- for as long as it stayed in the schedule's past. A row here is the only thing
-- standing between one missed service and five identical e-mails about it.
--
-- `scope` is the event vocabulary ('failure' | 'missed' | 'receipt'), `key`
-- identifies the occurrence within it, and the policy per scope is
-- `sundayrec_core::relay::seen_decision` — a ten-minute window for failures
-- (repeatable news), once and for all for the other two (a moment in the past
-- cannot happen twice).
create table if not exists notify_seen (
  scope   TEXT NOT NULL,
  key     TEXT NOT NULL,
  seen_at INTEGER NOT NULL,              -- unix ms
  PRIMARY KEY (scope, key)
);

-- The ledger is swept by age, not by count: a key names an occurrence, so the
-- rows that may be forgotten are the ones whose occurrence is far enough in the
-- past that it cannot be re-reported.
create index if not exists idx_notify_seen_age
  on notify_seen (seen_at);
