/**
 * Tre navn, og tallene som ligger under dem.
 *
 * Atlasets §3c teller FEM steder mix/master kan skje i dagens app — Normaliser,
 * mastring-panelet, eksportmodalens mastring, ett-klikks lydforbedring og den
 * avanserte mikseren — med to helt ulike utfall (en NY fil ved siden av
 * originalen, eller behandling i eksporten) og ingenting i UI-et som forklarer
 * forskjellen. Canvasens sett 4 gjør det om til ÉN bryter og TRE ord, og denne
 * fila er stedet de tre ordene blir til konkrete verdier.
 *
 * Den er ren og node-testet med vilje: den er en TABELL noen har lest gjennom,
 * ikke betingelser spredt utover en eksportfunksjon. Eieren skal kunne peke på
 * en linje her og si «nei, Tale skal være −19, ikke −16».
 *
 * ## Hvorfor mastring-presettet ALENE, og ingen stemmekjede
 *
 * Den nærliggende mappingen — `editor_auto_process` sin stemmekjede PLUSS et
 * mastring-preset — er nøyaktig den bakenden advarer mot, med ord:
 *
 *   > It deliberately does NOT recommend a mastering preset. Stacking one on
 *   > top of the vocal chain ran the material through two highpasses, two
 *   > compressors and two EQ curves — the classic over-processed «one-click»
 *   > result (pumping, thin low end).
 *   — `src-tauri/src/editor/mod.rs`, `auto_process`
 *
 * Og et mastring-preset ER en stemmekjede: `speech-clear` er
 * `highpass=f=80` + tre EQ-bånd + `acompressor`, og DERETTER loudnorm mot
 * −16 LUFS (`crates/sundayrec-core/src/mastering.rs`). Én kjede, én
 * kompressor, ett høypass — og et utgivelsesnivå, som er halve løftet i
 * «jevner ut nivået».
 *
 * Så: profilen er ett mastring-preset. Stemmekjeden finnes fortsatt, bak
 * «Avansert: åpne mikseren», og der ERSTATTER den profilen i stedet for å
 * legge seg oppå den.
 *
 * ## Kanalreparasjonen er ikke en smakssak
 *
 * Det ENE `editor_auto_process` gjør som presettet ikke gjør er å oppdage at
 * venstre kanal er stille eller at kanalene er ulike i styrke. Det er en
 * REPARASJON, ikke en farge: uten den eksporteres en halvstum fil, og det er
 * den vanligste ekte katastrofen i et menighetsopptak. Den rir derfor med på
 * begge profilene, og på mikseren — mikseren har ingen kanalkontroll.
 *
 * «Ingen» får den ikke: «Eksporter lyden slik den ble tatt opp» er et løfte,
 * og en stille kanal som plutselig høres er ikke det.
 */

import type { ExportChannelRepair } from "@lib/pages/editor/export-params";

/** De tre ordene. `none` er også det bryteren AV betyr. */
export type SoundProfile = "speech" | "mixed" | "none";

/** Rekkefølgen kortene står i. «Tale» er anbefalt og først. */
export const SOUND_PROFILES: readonly SoundProfile[] = [
  "speech",
  "mixed",
  "none",
];

/** Profilen en frivillig får uten å velge noe. */
export const DEFAULT_SOUND_PROFILE: SoundProfile = "speech";

/**
 * «Tale» → `speech-clear`, «Tale — tydelig (anbefalt)».
 *
 * −16 LUFS / LRA 8 / true peak −1 dBTP. Kjeden foran loudnorm er
 * `highpass=f=80`, −1,5 dB @ 200 Hz, +2 dB @ 3 kHz, −2 dB @ 7 kHz og en
 * kompressor på −18 dB / 2,5:1. Det er standardmålet for tale og podkast, og
 * det er presettet legacys mastring-panel selv forhåndsvelger.
 */
export const SPEECH_MASTER_PRESET = "speech-clear";

/**
 * «Tale og musikk» → `music-speech`, «Musikk + tale».
 *
 * Samme −16 LUFS, men LRA 11 i stedet for 8: lovsang som er ment å svinge fra
 * stille til sterkt skal FÅ svinge. Kjeden er mildere med vilje —
 * `highpass=f=50` (orgel og bass overlever) og en kompressor på −22 dB / 2:1.
 */
export const MIXED_MASTER_PRESET = "music-speech";

/** Hva en profil faktisk sender med eksporten. */
export interface SoundProfileSpec {
  /** Mastring-preset-id fra `sundayrec_core::mastering::master_presets()`. */
  masterPreset: string | undefined;
  /** Skal kanalreparasjonen fra `editor_auto_process` bli med? */
  channelRepair: boolean;
}

const SPECS: Record<SoundProfile, SoundProfileSpec> = {
  speech: { masterPreset: SPEECH_MASTER_PRESET, channelRepair: true },
  mixed: { masterPreset: MIXED_MASTER_PRESET, channelRepair: true },
  none: { masterPreset: undefined, channelRepair: false },
};

export function specFor(profile: SoundProfile): SoundProfileSpec {
  return SPECS[profile];
}

/** Feltene en eksport arver fra lyd-steget. */
export interface SoundExportFields {
  masterPreset?: string;
  vocalChainPreset?: string;
  processing?: unknown;
  channelRepair?: ExportChannelRepair;
}

export interface SoundState {
  profile: SoundProfile;
  /** Er den avanserte mikseren i bruk? Da ERSTATTER den profilen. */
  useMixer: boolean;
  /** Mikserens fulle `EditorProcessing`, når den er i bruk. */
  processing?: unknown;
  /** Reparasjonen `editor_auto_process` anbefalte, eller `null`. */
  repair: ExportChannelRepair | null;
}

/**
 * Lyd-steget → eksportnyttelastens lydfelter.
 *
 * Tre utfall, og ikke fire:
 *
 *   «Ingen»      → ingenting. Fila kommer ut slik den gikk inn.
 *   mikser på    → `processing`, og INGEN `masterPreset`. Den som har åpnet
 *                  mikseren har tatt over kjeden; å legge et preset under den
 *                  ville vært det doble høypasset igjen, bare med et annet
 *                  navn på døra.
 *   ellers       → profilens `masterPreset`.
 *
 * `vocalChainPreset` sendes ALDRI. Den finnes i nyttelasten fordi bakenden tar
 * imot den, men ingen vei gjennom dette skallet setter den: mikseren sender
 * hele kjeden eksplisitt, og profilen sender et mastring-preset. To måter å si
 * det samme på er to måter å bli uenige på.
 */
export function soundExportFields(state: SoundState): SoundExportFields {
  if (state.profile === "none") return {};
  const repair = state.repair ?? undefined;
  if (state.useMixer) {
    return { processing: state.processing, channelRepair: repair };
  }
  return {
    masterPreset: specFor(state.profile).masterPreset,
    channelRepair: repair,
  };
}

/**
 * Utsnittet før/etter-lyttingen bruker: 20 sekunder fra MIDTEN av prekenen.
 *
 * Midten, og ikke starten: det første minuttet av en preken er ofte en
 * innledning som er tatt opp mens folk setter seg, og det er ikke materialet
 * man vurderer en lydprofil på. Uten et prekenvindu er midten av det som blir
 * IGJEN etter kuttene det nærmeste vi kommer.
 *
 * Klemt slik at hele utsnittet ligger inne i fila — bakenden klemmer også
 * (`clamp_preview_start`), men et startpunkt som ligger 5 sekunder før slutten
 * gir 5 sekunders lytting og ingen beskjed om hvorfor.
 */
export const LISTEN_SPAN_SEC = 20;

export function listenStartSec(
  window: { start: number; end: number } | null,
  durationSec: number,
): number {
  if (!Number.isFinite(durationSec) || durationSec <= 0) return 0;
  const mid =
    window && window.end > window.start
      ? (window.start + window.end) / 2
      : durationSec / 2;
  const latest = Math.max(0, durationSec - LISTEN_SPAN_SEC);
  return Math.min(Math.max(0, mid - LISTEN_SPAN_SEC / 2), latest);
}

/**
 * Kanaldiagnosens kode → SUFFIKSET under `editor.` som forklarer den.
 *
 * De seks tekstene finnes fra før i alle sju språk (`editor.chanBalanced` …
 * `editor.chanMono`), og de sier akkurat det en frivillig trenger å vite:
 * «Venstre kanal er stille (sjekk kabel)».
 *
 * Suffikset og ikke hele nøkkelen, fordi flaten slår det opp med
 * `tDyn("editor", suffix)`: prefikset må være en literal
 * `check-i18n-keys.mjs` kan slå opp. En ukjent kode gir `null`, og da står det
 * ingenting — ikke en råkode på skjermen.
 */
const CHANNEL_CODE_KEYS: Record<string, string> = {
  balanced: "chanBalanced",
  imbalance: "chanImbalance",
  dead_left: "chanDeadLeft",
  dead_right: "chanDeadRight",
  both_dead: "chanBothDead",
  mono: "chanMono",
};

export function channelCodeKey(code: string | undefined): string | null {
  if (!code) return null;
  return CHANNEL_CODE_KEYS[code] ?? null;
}
