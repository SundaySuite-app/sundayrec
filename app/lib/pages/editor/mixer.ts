// The `Processing` DTO mirrors the Rust `EditorProcessing` (camelCase
// bindings), plus the two functions that build/read it. On export the whole
// object is sent as the `processing` override, which wins over
// `vocalChainPreset` server-side.
//
// F1-I18N-T: this file used to ALSO own the mixer's DOM rendering
// (`renderMixer`), the preset catalogue (`VOCAL_PRESETS`) and the loader that
// pushed a preset into the mixer and re-rendered (`loadPresetIntoMixer`) —
// legacy's hand-built `<input type="range">` panel, ported verbatim. `app/`
// replaced all of it with a Preact component (`app/editor/MixerPanel.tsx`,
// which keeps its own `STAGES`/`EQ_BANDS`/`SliderSpec`/`StageSpec`), and
// nothing in `app/` called the DOM-building trio any more — confirmed with a
// grep for each name before deleting. They also carried the mixer's entire
// stock of hardcoded Norwegian (stage titles, slider labels, preset names),
// which is exactly the kind of i18n leak this PR exists to close: dead code
// nobody sees is dead code nobody scans for hardcoded text, either.
//
// `Processing`/`defaultProcessing`/`mixerProcessing` stay: `defaultProcessing`
// is the one place `VocalChain::default()` is mirrored in TypeScript
// (`app/editor/sound.ts` seeds its `mixer` signal from it), and the
// `Processing` type is shared by `MixerPanel.tsx` and `sound.ts`.

import { E } from "./state";

// The processing object mirrors `EditorProcessing` (camelCase bindings).
export interface Processing {
  highpassEnabled: boolean;
  highpassHz: number;
  denoiseEnabled: boolean;
  denoiseDb: number;
  denoiseFloorDb: number;
  dereverbEnabled: boolean;
  dereverbStrength: number;
  gateEnabled: boolean;
  gateThresholdDb: number;
  gateRatio: number;
  eq: Array<{ freqHz: number; gainDb: number; q: number }>;
  compEnabled: boolean;
  compThresholdDb: number;
  compRatio: number;
  compAttackMs: number;
  compReleaseMs: number;
  compMakeupDb: number;
  deesserEnabled: boolean;
  deesserIntensity: number;
  limiterEnabled: boolean;
  limiterDb: number;
  gainDb: number;
}

// Mirror of `VocalChain::default()` — light polish (HPF + compressor on).
export function defaultProcessing(): Processing {
  return {
    highpassEnabled: true,
    highpassHz: 80,
    denoiseEnabled: false,
    denoiseDb: 12,
    denoiseFloorDb: -25,
    dereverbEnabled: false,
    dereverbStrength: 0.4,
    gateEnabled: false,
    gateThresholdDb: -40,
    gateRatio: 2,
    eq: [],
    compEnabled: true,
    compThresholdDb: -18,
    compRatio: 3,
    compAttackMs: 5,
    compReleaseMs: 80,
    compMakeupDb: 2,
    deesserEnabled: false,
    deesserIntensity: 0.4,
    limiterEnabled: false,
    limiterDb: -1,
    gainDb: 0,
  };
}

/** Build the export-ready `processing` object from the mixer state, dropping
 *  zero-gain EQ bands (a 0 dB equalizer is a wasted filter). */
export function mixerProcessing(): Processing {
  const p = { ...E.mixer };
  p.eq = p.eq.filter((b) => Math.abs(b.gainDb) > 0.01);
  return p;
}
