/**
 * live-stats-core — turning a `StreamStatus` into things a person can read.
 *
 * The Direkte page's statistics grid was gated «Kommer» for one honest reason:
 * the backend computed fps / bitrate / dropped several times a second and then
 * threw them away, because nothing emitted `streaming://stats`. That emitter
 * exists now, so the numbers are real — and the moment they are real, HOW they
 * are shown starts to matter:
 *
 *   - a stream that is not running shows «—», not `0 kbps`. Zero is a
 *     measurement; the absence of a stream is not.
 *   - per-destination health arrives from the backend keyed by NAME (the tee
 *     slave order), while the rows in the DOM are keyed by id. Joining those two
 *     wrong is how a working destination gets painted red.
 *   - a start failure arrives as a raw backend string. `feature_disabled:
 *     streaming.start requires a build with \`--features streaming\`` is a
 *     sentence for whoever compiled the app, and it was being shown to a
 *     volunteer twenty minutes before a service.
 *
 * All of that is pure, so it is here and it is tested.
 */

/** The live status the backend pushes (`StreamStatus`, camelCase over IPC). */
export interface StreamStatusView {
  active: boolean
  /** Epoch-ms the current stream started, or null when idle. */
  startedAt: number | null
  bitrateKbps: number
  fps: number
  dropped: number
  /** Last interesting line, already key-redacted and localized by the backend. */
  lastLine: string
  /** Per-destination liveness, in the tee's slave order. Empty when idle. */
  destinations: Array<{ name: string; ok: boolean }>
  /** The bitrate the encoder is currently TARGETING (0 when idle). */
  targetBitrateKbps: number
  /** Adaptive-bitrate degradation tier: 0 = full quality, 1–2 = stepped down. */
  bitrateStep: number
}

/** An idle status — the shape `stream_status` returns before anything starts. */
export function emptyStreamStatus(): StreamStatusView {
  return {
    active: false,
    startedAt: null,
    bitrateKbps: 0,
    fps: 0,
    dropped: 0,
    lastLine: '',
    destinations: [],
    targetBitrateKbps: 0,
    bitrateStep: 0,
  }
}

/** Placeholder for "there is no measurement", as distinct from "the measurement
 *  is zero". Used everywhere the stream is not running. */
export const NO_VALUE = '—'

// ── The four numbers ──────────────────────────────────────────────────────

/** `2500` → `"2500 kbps"`; idle → «—». */
export function formatBitrate(s: StreamStatusView): string {
  if (!s.active) return NO_VALUE
  return `${Math.max(0, Math.round(s.bitrateKbps))} kbps`
}

/** Encoder frames per second; idle → «—». */
export function formatFps(s: StreamStatusView): string {
  if (!s.active) return NO_VALUE
  return String(Math.max(0, Math.round(s.fps)))
}

/**
 * Dropped frames. Shown even when the stream has ENDED, because "how many
 * frames did we lose" is a question asked after the service at least as often
 * as during it — but only when a stream actually ran (`startedAt` was set or
 * frames were genuinely lost), so an untouched page shows «—» rather than a
 * confident 0.
 */
export function formatDropped(s: StreamStatusView): string {
  if (!s.active && s.startedAt === null && s.dropped === 0) return NO_VALUE
  return String(Math.max(0, Math.round(s.dropped)))
}

/**
 * Wall-clock time since the stream started, `mm:ss` (or `h:mm:ss` past an
 * hour — a 90-minute service would otherwise read `94:12`).
 *
 * `nowMs` is passed in so this stays pure and the test doesn't race a clock.
 */
export function formatUptime(s: StreamStatusView, nowMs: number): string {
  if (!s.active || s.startedAt === null) return '00:00'
  const total = Math.max(0, Math.floor((nowMs - s.startedAt) / 1000))
  const ss = String(total % 60).padStart(2, '0')
  const mm = Math.floor(total / 60) % 60
  const hh = Math.floor(total / 3600)
  if (hh > 0) return `${hh}:${String(mm).padStart(2, '0')}:${ss}`
  return `${String(mm).padStart(2, '0')}:${ss}`
}

// ── The status pill ───────────────────────────────────────────────────────

/** The pill's CSS state class. */
export type LivePillState = 'is-idle' | 'is-preparing' | 'is-live' | 'is-error'

/**
 * What the pill should say for a status.
 *
 * The old subscriber only ever SET the pill when the stream was active, or when
 * an inactive status happened to carry a `lastLine` — so a stream that stopped
 * cleanly left a red «Live» pill sitting there. Every status maps to a state
 * here, with no branch that leaves the previous one standing.
 */
export function livePillState(s: StreamStatusView): LivePillState {
  if (s.active) return 'is-live'
  // Not active, but the backend left an explanation behind → it did not stop
  // because someone asked it to.
  if (s.lastLine.trim()) return 'is-error'
  return 'is-idle'
}

/**
 * Whether the adaptive-bitrate supervisor has quietly degraded the stream.
 * `bitrateStep > 0` means ffmpeg was respawned at a lower tier to survive a
 * congested uplink — worth saying, because the alternative is a stream that
 * silently looks worse than the quality the operator selected.
 */
export function isQualityReduced(s: StreamStatusView): boolean {
  return s.active && s.bitrateStep > 0
}

// ── Per-destination health ────────────────────────────────────────────────

/** A destination row as the page rendered it. */
export interface LiveDestinationRow {
  id: string
  name: string
  /** Toggled on for this session. */
  enabled: boolean
}

/** The dot states the stylesheet knows (`.live-dest-dot[data-state=…]`). */
export type LiveDestState = 'disabled' | 'idle' | 'connecting' | 'live' | 'failed'

/**
 * Join the backend's per-destination health onto the rendered rows.
 *
 * The backend reports `{ name, ok }` in tee-slave order; the DOM keys rows by
 * id. The previous renderer read `{ id, state }` off the status — fields that
 * have never existed on `StreamStatus` — so `dot.dataset.state` was set to
 * `undefined` for every destination on every event. Nothing threw; the dots
 * just never moved.
 *
 * Matching is by name, consuming each health entry once so two destinations
 * sharing a name still map in order. An enabled row with no health entry (the
 * backend found no stream key for it in the keychain, so it was never pushed
 * to) stays `idle` rather than claiming to be live.
 */
export function mapDestinationStates(
  rows: LiveDestinationRow[],
  s: StreamStatusView,
): Map<string, LiveDestState> {
  const out = new Map<string, LiveDestState>()
  const taken = new Set<number>()
  for (const row of rows) {
    if (!row.enabled) {
      out.set(row.id, 'disabled')
      continue
    }
    if (!s.active) {
      out.set(row.id, 'idle')
      continue
    }
    const idx = s.destinations.findIndex((d, i) => !taken.has(i) && d.name === row.name)
    if (idx === -1) {
      out.set(row.id, 'idle')
      continue
    }
    taken.add(idx)
    out.set(row.id, s.destinations[idx].ok ? 'live' : 'failed')
  }
  return out
}

/** How many enabled destinations the backend believes are still receiving. */
export function liveDestinationCount(s: StreamStatusView): number {
  return s.destinations.filter(d => d.ok).length
}

// ── Start failures ────────────────────────────────────────────────────────

/**
 * Why `stream_start` refused, as a stable code the page turns into a sentence.
 *
 * The raw strings come from `AppError` — `feature_disabled: streaming.start
 * requires a build with \`--features streaming\``, `no_camera`,
 * `stream_already_active`, `invalid_destination:<id>:<reason>`, `stream ffmpeg
 * spawn: …`. Every one of them was being rendered verbatim into the error strip
 * under the START button.
 */
export type LiveStartErrorCode =
  | 'featureDisabled'
  | 'noCamera'
  | 'alreadyActive'
  | 'invalidDestination'
  | 'spawnFailed'
  | 'unknown'

export function classifyStartError(raw: string): LiveStartErrorCode {
  const m = (raw ?? '').toLowerCase()
  if (m.includes('feature_disabled')) return 'featureDisabled'
  if (m.includes('no_camera')) return 'noCamera'
  if (m.includes('stream_already_active')) return 'alreadyActive'
  if (m.includes('invalid_destination') || m.includes('stream_args')) return 'invalidDestination'
  if (m.includes('ffmpeg spawn')) return 'spawnFailed'
  return 'unknown'
}
