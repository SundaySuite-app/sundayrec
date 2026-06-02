// Peak (waveform) computation + normalization helpers for the editor.
// Ported from the Electron renderer (`editor/peaks.ts`). The functions are pure
// (state passed in) so they unit-test without a DOM or AudioContext.

import type { EditorState } from "./types";

/**
 * Synchronous peak computation at 100 Hz. Used for short/normal files where the
 * cost is negligible. Long files come in pre-downsampled via the ffmpeg-extract
 * path (8 kHz mono), so even a 4 h recording is only ~115 M samples here. Also
 * records clipping timestamps (peak ≥ 0.99) into `state.clipTimes`.
 */
export function computePeaks(
  state: EditorState,
  buf: AudioBuffer,
): Float32Array {
  const RATE = 100;
  const total = Math.ceil(buf.duration * RATE);
  const out = new Float32Array(total);
  const ch0 = buf.getChannelData(0);
  const ch1 = buf.numberOfChannels > 1 ? buf.getChannelData(1) : ch0;
  const spp = Math.floor(buf.sampleRate / RATE);
  state.clipTimes = [];

  for (let i = 0; i < total; i++) {
    const s = i * spp;
    const e = Math.min(s + spp, ch0.length);
    let pk = 0;
    for (let j = s; j < e; j++) {
      const v = Math.max(Math.abs(ch0[j]), Math.abs(ch1[j]));
      if (v > pk) pk = v;
    }
    out[i] = pk;
    if (pk >= 0.99) state.clipTimes.push(i / RATE);
  }
  return out;
}

/** Peaks for an intro/outro jingle buffer — no clip tracking (dimmed display). */
export function computeJinglePeaks(buf: AudioBuffer): Float32Array {
  const RATE = 100;
  const total = Math.ceil(buf.duration * RATE);
  const out = new Float32Array(total);
  const ch0 = buf.getChannelData(0);
  const ch1 = buf.numberOfChannels > 1 ? buf.getChannelData(1) : ch0;
  const spp = Math.max(1, Math.floor(buf.sampleRate / RATE));
  for (let i = 0; i < total; i++) {
    const s = i * spp;
    const e = Math.min(s + spp, ch0.length);
    let pk = 0;
    for (let j = s; j < e; j++) {
      const v = Math.max(Math.abs(ch0[j]), Math.abs(ch1[j]));
      if (v > pk) pk = v;
    }
    out[i] = pk;
  }
  return out;
}

/**
 * Gain (dB) to bring the maximum absolute peak in `pks` to −1 dBFS (1 dB of
 * headroom). Returns 0 if silent or already at/above target. Peaks are floats
 * in 0..1; a >1 fallback guards against a future uint8 caller.
 */
export function computePeakGain(pks: Float32Array): number {
  let max = 0;
  for (let i = 0; i < pks.length; i++) {
    const v = Math.abs(pks[i]);
    if (v > max) max = v;
  }
  if (max <= 0) return 0;
  const normalizedMax = max > 1.001 ? max / 128 : max;
  const currentDb = 20 * Math.log10(normalizedMax);
  if (currentDb >= -1) return 0;
  return -1 - currentDb;
}

/** Linear gain factor for the current `audioGainDb` (1.0 = no change). */
export function gainFactor(state: EditorState): number {
  return state.audioGainDb === 0 ? 1 : Math.pow(10, state.audioGainDb / 20);
}
