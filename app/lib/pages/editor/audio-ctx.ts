// ── The editor's ONE AudioContext ───────────────────────────────────────────
//
// The main recording no longer touches Web Audio at all — it streams through a
// media element (see state.playerEl). What is left is the short intro/outro
// jingles, which we still decode to AudioBuffers so they can be scheduled
// sample-accurately against the element transport.
//
// Before this module every decode opened its OWN `new AudioContext()`: one per
// jingle in loadIntroOutroBuffers (2) plus one per main-file decode path — up to
// four live hardware contexts per opened file, of which only one was ever
// closed. macOS caps a process at a handful of contexts, so a few file swaps
// left the editor unable to create any at all (silent playback, no error).
//
// One lazily-created context for the app's lifetime instead. It is deliberately
// NEVER closed: every decoded AudioBuffer belongs to the context that decoded
// it, so closing would invalidate the jingle buffers still referenced by E.

let ctx: AudioContext | null = null;

/**
 * The shared context, created on first use. Also resumes it — WKWebView starts
 * a context `suspended` until a user gesture, and a suspended context schedules
 * nothing (the jingle would simply not sound).
 */
export function sharedAudioCtx(): AudioContext {
  if (!ctx) ctx = new AudioContext();
  if (ctx.state === "suspended") void ctx.resume().catch(() => {});
  return ctx;
}

/** The shared context ONLY if one already exists — never creates one. Position
 *  readers run on every animation frame and must not conjure hardware. */
export function peekSharedAudioCtx(): AudioContext | null {
  return ctx;
}
