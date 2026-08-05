/**
 * Audio device + routing helpers (renderer-side).
 *
 * Recording is handled entirely by the native Rust capture engine
 * (src-tauri/src/recorder) — this module never opens a monitoring stream for
 * the recording itself. What's left, and still live, is what the VU meters on
 * Home/Live/Onboarding actually call:
 *   • getAudioDevices()  — enumerate mic inputs for device pickers
 *   • rVuChannel()       — which splitter output feeds the RIGHT VU analyser
 *   • buildInputRouter() — route an arbitrary device channel pair to a
 *     2-channel node for metering (custom channel maps on digital mixers)
 *
 * A renderer crash no longer affects the recording.
 */

// ── Device helpers ───────────────────────────────────────────────────────────

export async function getAudioDevices(): Promise<MediaDeviceInfo[]> {
  try {
    const s = await navigator.mediaDevices.getUserMedia({ audio: true, video: false })
    s.getTracks().forEach(t => t.stop())
    return (await navigator.mediaDevices.enumerateDevices()).filter(d => d.kind === 'audioinput')
  } catch { return [] }
}

// ── VU meter channel helper ──────────────────────────────────────────────────

/**
 * Which channel-splitter output should drive the RIGHT VU analyser.
 *
 * WKWebView's `getUserMedia` commonly delivers a MONO track — most microphones
 * (and the built-in Mac mic) are mono, and it doesn't reliably honour
 * `channelCount: { ideal: 2 }`. Feeding such a stream into a 2-channel splitter
 * leaves output 1 (R) silent, so the meter shows only L even when "Stereo" is
 * selected (the reported bug). A Stereo recording of a mono source is dual-mono
 * (the backend ffmpeg duplicates L→R via `-ac 2`), so the meter SHOULD show both
 * bars. We therefore mirror channel 0 into R unless the device DEFINITIVELY
 * reports ≥2 channels (a real stereo interface), which keeps independent L/R.
 */
export function rVuChannel(stream: MediaStream): number {
  const ch = stream.getAudioTracks()[0]?.getSettings().channelCount ?? 0
  return ch >= 2 ? 1 : 0
}

// ── Channel routing helper (also used by audio-page.ts) ─────────────────────

export function buildInputRouter(
  ctx: AudioContext,
  src: MediaStreamAudioSourceNode,
  stream: MediaStream,
  chL: number,
  chR: number
): AudioNode {
  const actualCh = stream.getAudioTracks()[0]?.getSettings().channelCount ?? 2
  if (actualCh <= 2 && chL === 0 && chR === 1) return src
  const n        = Math.max(actualCh, Math.max(chL, chR) + 1)
  const splitter = ctx.createChannelSplitter(n)
  const merger   = ctx.createChannelMerger(2)
  src.connect(splitter)
  splitter.connect(merger, Math.min(chL, actualCh - 1), 0)
  splitter.connect(merger, Math.min(chR, actualCh - 1), 1)
  return merger
}
