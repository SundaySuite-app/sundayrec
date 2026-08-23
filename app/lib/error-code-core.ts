/**
 * The stable snake_code of a backend error — the ONE way the renderer is
 * allowed to pattern-match errors (R3-C).
 *
 * Rust's `AppError` crosses the IPC boundary as `{ code, message }`, where
 * `code` is the coarse category (`"validation"`, `"recording"`, …) and
 * `message` is the `Display` string `"<category>: <detail>"`. Every branchable
 * detail leads with a stable snake code (`no_save_folder`, `feature_disabled`,
 * `cancelled`, `no_config_smtp_host`, …) followed by optional prose. Before
 * this helper, call sites substring-matched English PROSE out of those
 * messages (`err.includes('no_config: smtp host')`,
 * `msg.endsWith("cancelled")`) — matching that breaks the day someone rewords
 * a Rust format string.
 *
 * `errorCode` extracts the leading snake code: strip the category prefix, take
 * the first `[a-z0-9_]` token. Returns `""` when there is none (a raw OS/ffmpeg
 * message) — callers then fall back to showing the message verbatim.
 */

/** The category prefixes `AppError`'s `Display` impl produces (error.rs). */
const CATEGORY_PREFIX =
  /^(?:not found|validation|recording error|audio error|io error|invalid json|database error|migration error|internal): /

/** Message text from anything a rejected invoke (or an `{error}` field) holds. */
function messageOf(e: unknown): string {
  if (typeof e === 'string') return e
  if (e instanceof Error) return e.message
  if (e && typeof e === 'object') {
    const o = e as Record<string, unknown>
    if (typeof o.message === 'string' && o.message) return o.message
    if (typeof o.error === 'string' && o.error) return o.error
    if (typeof o.code === 'string' && o.code) return o.code
  }
  return ''
}

/**
 * The leading stable snake code of a backend error, or `''` when the message
 * doesn't lead with one. Accepts the raw rejection object, an `Error`, or an
 * already-extracted message/error string.
 */
export function errorCode(e: unknown): string {
  const stripped = messageOf(e).replace(CATEGORY_PREFIX, '')
  // The trailing lookahead includes `_`, so a partial backtrack can never
  // "find" a shorter code inside a longer word (`no_configX` is NOT `no`).
  const m = /^([a-z][a-z0-9_]*)(?![a-zA-Z0-9_])/.exec(stripped)
  return m ? m[1] : ''
}
