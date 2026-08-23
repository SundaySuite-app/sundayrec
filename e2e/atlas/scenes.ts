import { expect, type Page } from "@playwright/test";
import {
  BOOT_FIXTURES,
  fn,
  recordingRow,
  VOID,
  type Fixtures,
} from "../harness";
import type { BootOptions } from "../harness";
import { emit, settleVu, LAST_SUNDAY_MS, NEXT_SERVICE_ISO } from "./harness";

// THE SCENE TABLE — every screen and state a volunteer can reach, as of the
// «Frivilligen først» R-phase (PR #139 + #141 removed cloud, podcast, webhooks,
// integrations, whisper, sermon-help, chapters and learning cards).
//
// A scene is a recipe, not an assertion: fixtures + settings + a few clicks +
// the selector that says "this screen has arrived". The spec drives it once per
// locale and writes a PNG. If a scene cannot be reached, the run says so out
// loud rather than photographing whatever was on screen.

export interface Scene {
  /** Filename stem: `<page>--<state>`. */
  id: string;
  /** Human page name, for INDEX.md. */
  page: string;
  /** Human state name, for INDEX.md. */
  state: string;
  /** One word: the fixture or trick that produces this state. */
  recipe: string;
  boot: BootOptions;
  /** Selector that must be visible before the shot. */
  wait?: string;
  /** Clicks / events between boot and the shot. */
  act?: (page: Page) => Promise<void>;
  /** Also take a `fullPage` shot (the screen scrolls past the window). */
  full?: boolean;
  /** Also take a 960×640 shot — the app's minimum window (no-locale only). */
  small?: boolean;
}

// ── Shared fixture material ──────────────────────────────────────────────────

/** A mixer and a laptop mic: the two device classes the picker distinguishes. */
const DEVICES = [
  {
    id: "Allen & Heath Qu-5",
    name: "Allen & Heath Qu-5",
    backend: "coreaudio",
    inputChannels: 32,
    sampleRates: [44_100, 48_000, 96_000],
    isDefault: false,
  },
  {
    id: "MacBook Pro Microphone",
    name: "MacBook Pro Microphone",
    backend: "coreaudio",
    inputChannels: 2,
    sampleRates: [48_000],
    isDefault: true,
  },
];

const MIXER = "Allen & Heath Qu-5";
const SAVE_FOLDER = "/Users/frivillig/Opptak";

/** Sunday 11:00–12:30 every week — the shape a church actually configures. */
const SUNDAY_SLOT = { days: [6], start: "11:00", stop: "12:30", max: 120 };

/** Settings for an install that has been through first-run and works. */
const READY = {
  onboardingDone: true,
  deviceId: MIXER,
  deviceName: MIXER,
  saveFolder: SAVE_FOLDER,
  churchName: "Alta Frikirke",
  responsiblePerson: "Kari Nordmann",
};

/** Five takes with distinct shapes: long, short, video, noted, odd name. */
const RECORDINGS = [
  recordingRow({
    id: "rec-1",
    file_path: `${SAVE_FOLDER}/2026-08-16 Gudstjeneste.mp3`,
    device_name: MIXER,
    started_at: LAST_SUNDAY_MS,
    created_at: LAST_SUNDAY_MS,
    duration_ms: 5_400_000,
    byte_size: 172_000_000,
  }),
  recordingRow({
    id: "rec-2",
    file_path: `${SAVE_FOLDER}/2026-08-13 Bønnemøte.mp3`,
    device_name: MIXER,
    started_at: LAST_SUNDAY_MS - 3 * 86_400_000,
    created_at: LAST_SUNDAY_MS - 3 * 86_400_000,
    duration_ms: 900_000,
    byte_size: 28_000_000,
    note: "Kort møte — Anne leste teksten",
  }),
  recordingRow({
    id: "rec-3",
    file_path: `${SAVE_FOLDER}/2026-08-09 Familiegudstjeneste med dåp og nattverd.mp4`,
    device_name: MIXER,
    started_at: LAST_SUNDAY_MS - 7 * 86_400_000,
    created_at: LAST_SUNDAY_MS - 7 * 86_400_000,
    duration_ms: 4_500_000,
    byte_size: 2_100_000_000,
  }),
  recordingRow({
    id: "rec-4",
    file_path: `${SAVE_FOLDER}/2026-08-02 Gudstjeneste.mp3`,
    device_name: MIXER,
    started_at: LAST_SUNDAY_MS - 14 * 86_400_000,
    created_at: LAST_SUNDAY_MS - 14 * 86_400_000,
    duration_ms: 3_900_000,
    byte_size: 124_000_000,
  }),
  recordingRow({
    id: "rec-5",
    file_path: `${SAVE_FOLDER}/2026-07-26 Gudstjeneste.mp3`,
    device_name: MIXER,
    started_at: LAST_SUNDAY_MS - 21 * 86_400_000,
    created_at: LAST_SUNDAY_MS - 21 * 86_400_000,
    duration_ms: 3_600_000,
    byte_size: 115_000_000,
  }),
];

const TRASHED = [
  {
    id: "t1",
    originalPath: `${SAVE_FOLDER}/2026-07-19 Gudstjeneste.mp3`,
    trashedPath: "/tmp/trash/2026-07-19.mp3",
    name: "2026-07-19 Gudstjeneste.mp3",
    deletedAt: LAST_SUNDAY_MS - 20 * 86_400_000,
    related: [],
    byteSize: 118_000_000,
  },
];

/**
 * A brand-new install: every boot command answers, and every answer is empty.
 * The version is the shipped one rather than the harness's `0.10.0-e2e` — the
 * sidebar renders it, and an atlas that says «Beta 10» dates itself wrong.
 */
const COLD: Fixtures = { ...BOOT_FIXTURES, app_info: { version: "0.15.0" } };

/** The fixtures a populated, working install answers with. */
const LIVE: Fixtures = {
  ...COLD,
  list_audio_devices: DEVICES,
  list_devices: { video_inputs: [{ name: "FaceTime HD Camera", index: 0 }] },
  scheduler_status: { next: NEXT_SERVICE_ISO },
  recordings_list: RECORDINGS,
  start_vu: 32,
  stop_vu: VOID,
};

const RECORDER: Fixtures = {
  ...LIVE,
  plan_recording_opts: { planned: true },
  start_recording: null,
  stop_recording: true,
};

const DIAGNOSE: Fixtures = {
  ...LIVE,
  diagnose_audio: {
    dshow: [MIXER, "MacBook Pro Microphone"],
    wasapi: [],
    wasapiAvailable: false,
  },
  media_permissions: { camera: "authorized", microphone: "authorized" },
  ffmpeg_health: { available: true, version: "7.1.1", path: "/opt/ffmpeg" },
  run_diagnostics: {
    markdown:
      "# SundayRec-diagnose\n\n- App: 0.15.0\n- OS: macOS 26.6 (aarch64)\n- Lydenhet: Allen & Heath Qu-5 (32 kanaler)\n- ffmpeg: 7.1.1\n- Ledig plass: 250 GB\n",
    findings: [],
    savedTo: `${SAVE_FOLDER}/diagnose-2026-08-23.md`,
  },
};

// ── Editor material ──────────────────────────────────────────────────────────

const EDITOR_FILE = `${SAVE_FOLDER}/2026-08-16 Gudstjeneste.mp3`;
const EDITOR_DURATION = 3_600;

const EDITOR_SEGMENTS = [
  { start: 0, end: 240, duration: 240, label: "Stillhet", type: "silence" },
  { start: 240, end: 900, duration: 660, label: "Musikk", type: "music" },
  { start: 900, end: 1_140, duration: 240, label: "Tale", type: "speech" },
  { start: 1_140, end: 1_500, duration: 360, label: "Musikk", type: "music" },
  {
    start: 1_500,
    end: 3_180,
    duration: 1_680,
    label: "Preken",
    type: "sermon",
  },
  { start: 3_180, end: 3_600, duration: 420, label: "Musikk", type: "music" },
];

const MASTER_PRESETS = [
  {
    id: "speech-natural",
    label: "Tale — naturlig",
    description: "−19 LUFS, mild komprimering. Nærmest råopptaket.",
    targetLufs: -19,
    targetLra: 7,
    truePeakDb: -1.5,
    filters: "loudnorm",
  },
  {
    id: "speech-clear",
    label: "Tale — tydelig",
    description: "−16 LUFS. Standard for tale på nett.",
    targetLufs: -16,
    targetLra: 6,
    truePeakDb: -1.5,
    filters: "loudnorm",
  },
];

function editorFixtures(over: Fixtures = {}): Fixtures {
  return {
    ...LIVE,
    editor_probe_streams: { hasVideo: false, hasAudio: true },
    editor_load_recording: {
      durationSec: EDITOR_DURATION,
      hasVideo: false,
      hasAudio: true,
      channels: 2,
      sampleFmt: "s16",
      sampleRate: 48_000,
    },
    editor_allow_asset_path: VOID,
    // Deterministic pseudo-waveform: loud where the segments say speech/music,
    // quiet in the silence. Generated in the page — a 360 000-element literal
    // does not belong on the init-script boundary.
    editor_peaks: fn(`() => ({
      peaks: Array.from({ length: ${EDITOR_DURATION} * 100 }, (_, i) => {
        const s = i / 100;
        const q = s < 240 || (s > 3180 && s < 3200);
        const base = q ? 0.02 : 0.55;
        return Math.min(1, Math.abs(base + 0.35 * Math.sin(s / 3) * Math.sin(s / 17)));
      }),
      sampleRate: 8000,
    })`),
    editor_segments: EDITOR_SEGMENTS,
    editor_read_sidecar: null,
    editor_write_sidecar: true,
    editor_delete_sidecar: true,
    editor_master_presets: MASTER_PRESETS,
    editor_probe_peak: -3.2,
    editor_cleanup_temp_files: 0,
    ...over,
  };
}

async function openEditor(page: Page): Promise<void> {
  await page.evaluate(
    (f) =>
      (
        window as unknown as { openEditorWithFile: (p: string) => void }
      ).openEditorWithFile(f),
    EDITOR_FILE,
  );
  await expect(page.locator("#editor-workspace")).toBeVisible();
}

// ── The scenes ───────────────────────────────────────────────────────────────

export const SCENES: Scene[] = [
  // ══ HJEM ═══════════════════════════════════════════════════════════════════
  {
    id: "home--kald-forstegangs",
    page: "Hjem",
    state: "Kald app: ingen enhet valgt, ingen lagringsmappe, ingen tidsplan",
    recipe: "COLD",
    boot: { fixtures: COLD, settings: { onboardingDone: true }, goto: "home" },
    wait: "#page-home",
    full: true,
    small: true,
  },
  {
    id: "home--klar-med-enhet",
    page: "Hjem",
    state:
      "Klar: mikser koblet til, neste gudstjeneste planlagt, opptak i historikken",
    recipe: "LIVE",
    boot: {
      fixtures: LIVE,
      settings: { ...READY, slots: [SUNDAY_SLOT] },
      goto: "home",
    },
    wait: "#hero-ok",
    full: true,
  },
  {
    id: "home--nivaa-live",
    page: "Hjem",
    state: "Lydnivå live — VU-målerne får ekte pakker fra vu://levels",
    recipe: "vu://levels",
    boot: {
      fixtures: LIVE,
      settings: { ...READY, slots: [SUNDAY_SLOT] },
      goto: "home",
    },
    wait: "#hero-ok",
    act: async (page) => {
      const delivered = await settleVu(page, "vu://levels", -17, -8);
      expect(delivered, "ingen abonnenter på vu://levels").toBeGreaterThan(0);
      await expect(page.locator("#signal-text")).not.toHaveText("—");
    },
  },
  {
    id: "home--enhet-borte",
    page: "Hjem",
    state: "Lagret mikser finnes ikke lenger — hero-advarsel «Koble til …»",
    recipe: "list_audio_devices:[]",
    boot: {
      fixtures: { ...LIVE, list_audio_devices: [] },
      settings: { ...READY, slots: [SUNDAY_SLOT] },
      goto: "home",
    },
    wait: "#hero-warn",
  },
  {
    id: "home--lite-diskplass",
    page: "Hjem",
    state: "0,6 GB ledig — lagringskortet blir rødt",
    recipe: "get_disk_space",
    boot: {
      fixtures: {
        ...LIVE,
        get_disk_space: { freeBytes: 600_000_000, totalBytes: 500_000_000_000 },
      },
      settings: { ...READY, slots: [SUNDAY_SLOT] },
      goto: "home",
    },
    wait: "#home-storage-value",
  },
  {
    id: "home--forhandssjekk",
    page: "Hjem",
    state: "Pre-start-sjekken fant feil og advarsel (30 min før start)",
    recipe: "scheduler://preflight",
    boot: {
      fixtures: LIVE,
      settings: { ...READY, slots: [SUNDAY_SLOT] },
      goto: "home",
    },
    act: async (page) => {
      const n = await emit(page, "scheduler://preflight", [
        {
          severity: "error",
          category: "device",
          message: "Lydenheten «Allen & Heath Qu-5» er ikke koblet til.",
        },
        {
          severity: "warning",
          category: "disk",
          message: "Under 5 GB ledig plass i lagringsmappen.",
        },
      ]);
      expect(n, "ingen abonnenter på scheduler://preflight").toBeGreaterThan(0);
      await expect(page.locator("#preflight-card")).toBeVisible();
    },
    full: true,
  },
  {
    id: "home--tapt-opptak",
    page: "Hjem",
    state: "Et planlagt opptak ble aldri tatt — kort + rød banner",
    recipe: "scheduler://missed",
    boot: {
      fixtures: LIVE,
      settings: { ...READY, slots: [SUNDAY_SLOT] },
      goto: "home",
    },
    act: async (page) => {
      const n = await emit(page, "scheduler://missed", [
        { at: "2026-08-16T11:00:00", label: "Gudstjeneste" },
      ]);
      expect(n, "ingen abonnenter på scheduler://missed").toBeGreaterThan(0);
      await expect(page.locator("#missed-card")).toBeVisible();
    },
    full: true,
  },
  {
    id: "home--backend-feil",
    page: "Hjem",
    state: "Terminal opptaksfeil — global feilstripe øverst",
    recipe: "recording://error",
    boot: { fixtures: RECORDER, settings: { ...READY }, goto: "home" },
    act: async (page) => {
      const n = await emit(page, "recording://error", {
        code: "device_disconnected",
        message: "avfoundation: Input/output error",
      });
      expect(n, "ingen abonnenter på recording://error").toBeGreaterThan(0);
      await expect(page.locator("#global-error-banner")).toBeVisible();
    },
  },
  {
    id: "home--kvalitetsalarm",
    page: "Hjem",
    state: "Fila mangler lyd — datatap-banner med «Vis opptak»",
    recipe: "recording://quality",
    boot: { fixtures: RECORDER, settings: { ...READY }, goto: "home" },
    act: async (page) => {
      const n = await emit(page, "recording://quality", {
        expectedSec: 5_400,
        measuredSec: 3_120,
        reasons: ["input_overflow", "device_reopened"],
      });
      expect(n, "ingen abonnenter på recording://quality").toBeGreaterThan(0);
      await expect(page.locator(".ui-banner").first()).toBeVisible();
    },
  },
  {
    id: "home--samtykkekort",
    page: "Hjem",
    state: "Engangsspørsmålet om diagnostikk (needsPrompt)",
    recipe: "telemetry_consent_get",
    boot: {
      fixtures: {
        ...LIVE,
        telemetry_consent_get: {
          status: "neverAsked",
          version: 0,
          decidedAt: null,
          currentVersion: 2,
          needsPrompt: true,
          active: false,
        },
      },
      settings: { ...READY },
      goto: "home",
    },
    wait: "#telemetry-consent-toast",
  },
  {
    id: "home--video-pa",
    page: "Hjem",
    state: "Video slått på — kamerastripe og forhåndsvisning",
    recipe: "videoEnabled",
    boot: {
      fixtures: LIVE,
      settings: {
        ...READY,
        videoEnabled: true,
        videoDeviceName: "FaceTime HD Camera",
        videoDeviceIndex: 0,
        slots: [SUNDAY_SLOT],
      },
      goto: "home",
    },
    // NOT `#video-info-strip`: turning video on makes `relocateVuForVideoMode()`
    // physically MOVE the VU section, the preview and the two info cards into
    // `#video-mode-layout`'s slots — the original strip is left empty and
    // hidden. Same DOM nodes, different parent. Worth knowing before a redesign
    // touches this page.
    wait: "#video-mode-layout",
    full: true,
  },
  {
    id: "home--start-dialog",
    page: "Hjem",
    state: "«Start opptak nå»-dialogen",
    recipe: "modal-manual",
    boot: { fixtures: RECORDER, settings: { ...READY }, goto: "home" },
    act: async (page) => {
      await page.locator("#btn-start-recording").click();
      await expect(page.locator("#modal-manual")).toBeVisible();
    },
  },
  {
    id: "home--start-dialog-video",
    page: "Hjem",
    state: "«Start opptak nå» med video slått på — kameravalg i dialogen",
    recipe: "modal-manual+video",
    boot: {
      fixtures: RECORDER,
      settings: {
        ...READY,
        videoEnabled: true,
        videoDeviceName: "FaceTime HD Camera",
        videoDeviceIndex: 0,
      },
      goto: "home",
    },
    act: async (page) => {
      await page.locator("#btn-start-recording").click();
      await expect(page.locator("#modal-manual")).toBeVisible();
    },
  },
  {
    id: "opptak--pagar",
    page: "Opptaksoverlegg",
    state: "Opptak pågår, nivåer fra recording://levels",
    recipe: "start_recording",
    boot: { fixtures: RECORDER, settings: { ...READY }, goto: "home" },
    act: async (page) => {
      await page.locator("#btn-start-recording").click();
      await page.locator("#btn-manual-start").click();
      await expect(page.locator("#recording-overlay")).toBeVisible();
      const delivered = await settleVu(page, "recording://levels", -16, -7);
      expect(
        delivered,
        "ingen abonnenter på recording://levels",
      ).toBeGreaterThan(0);
    },
  },
  {
    id: "opptak--avbrudd",
    page: "Opptaksoverlegg",
    state: "Enheten falt ut — gjenkoblingsbanner + stillhetsvarsel",
    recipe: "recording://reconnecting",
    boot: { fixtures: RECORDER, settings: { ...READY }, goto: "home" },
    act: async (page) => {
      await page.locator("#btn-start-recording").click();
      await page.locator("#btn-manual-start").click();
      await expect(page.locator("#recording-overlay")).toBeVisible();
      await emit(page, "recording://reconnecting", {});
      await emit(page, "recording://silence", {
        code: "silence_detected",
        message: "Stillhet oppdaget i lydsignalet",
      });
      await expect(page.locator("#rec-reconnect")).toBeVisible();
      await expect(page.locator("#rec-silence")).toBeVisible();
    },
  },
  {
    id: "opptak--stopp-bekreftelse",
    page: "Opptaksoverlegg",
    state: "«Stopp opptak?»-dialogen (protectRecording er på som standard)",
    recipe: "modal-confirm-stop",
    boot: { fixtures: RECORDER, settings: { ...READY }, goto: "home" },
    act: async (page) => {
      await page.locator("#btn-start-recording").click();
      await page.locator("#btn-manual-start").click();
      await page.locator("#btn-stop-overlay").click();
      await expect(page.locator("#modal-confirm-stop")).toBeVisible();
    },
  },
  {
    id: "opptak--fullforer",
    page: "Opptaksoverlegg",
    state: "Etter bekreftet stopp: «Fullfører opptak …», knappen låst",
    recipe: "stop_recording",
    boot: { fixtures: RECORDER, settings: { ...READY }, goto: "home" },
    act: async (page) => {
      await page.locator("#btn-start-recording").click();
      await page.locator("#btn-manual-start").click();
      await page.locator("#btn-stop-overlay").click();
      await page.locator("#btn-confirm-stop").click();
      await expect(page.locator("#recording-overlay")).toHaveClass(
        /is-finalizing/,
      );
    },
  },

  // ══ TIDSPLAN ═══════════════════════════════════════════════════════════════
  {
    id: "schedule--tom",
    page: "Tidsplan",
    state: "Ingen faste tider, ingen enkeltopptak",
    recipe: "slots:[]",
    boot: { fixtures: LIVE, settings: { ...READY }, goto: "schedule" },
    wait: "#page-schedule",
    full: true,
    small: true,
  },
  {
    id: "schedule--med-tider",
    page: "Tidsplan",
    state: "Fast søndagstid + ett datert enkeltopptak",
    recipe: "slots+specials",
    boot: {
      fixtures: LIVE,
      settings: {
        ...READY,
        slots: [
          SUNDAY_SLOT,
          { days: [2], start: "19:00", stop: "20:30", max: null },
        ],
        specialRecordings: [
          {
            id: "sp-1",
            date: "2026-09-06",
            name: "Konfirmasjon",
            start: "10:30",
            stop: "12:30",
            deviceId: null,
          },
        ],
      },
      goto: "schedule",
    },
    wait: "#slots-list",
    full: true,
  },
  {
    id: "schedule--tid-editor",
    page: "Tidsplan",
    state: "Redigering av en fast tid (dagvelger, klokkeslett, maks lengde)",
    recipe: "#btn-add-slot",
    boot: {
      fixtures: LIVE,
      settings: { ...READY, slots: [SUNDAY_SLOT] },
      goto: "schedule",
    },
    act: async (page) => {
      await page.locator("#btn-add-slot").click();
      await expect(page.locator("#slot-editor")).toBeVisible();
    },
    full: true,
  },
  {
    id: "schedule--vekking-avansert",
    page: "Tidsplan",
    state: "«Avansert» utvidet: vekking fra dvale, strøm, søvnkonfig, test",
    recipe: "#btn-adv-toggle",
    boot: {
      fixtures: {
        ...LIVE,
        wake_capability: {
          supported: true,
          issues: [],
          platformNote: null,
        },
      },
      settings: { ...READY, slots: [SUNDAY_SLOT] },
      goto: "schedule",
    },
    act: async (page) => {
      await page.locator("#btn-adv-toggle").click();
      await expect(page.locator("#adv-section")).toBeVisible();
    },
    full: true,
  },
  {
    id: "schedule--dagsdetalj",
    page: "Tidsplan",
    state: "En kalenderdag valgt — hva som skjer den dagen",
    recipe: "kalenderklikk",
    boot: {
      fixtures: LIVE,
      settings: { ...READY, slots: [SUNDAY_SLOT] },
      goto: "schedule",
    },
    act: async (page) => {
      const day = page
        .locator("#cal-grid .cal-day:not(.cal-day-empty)")
        .nth(15);
      await day.click();
      await expect(page.locator("#cal-day-detail")).toBeVisible();
    },
  },

  // ══ INNSTILLINGER ══════════════════════════════════════════════════════════
  {
    id: "settings-audio--ingen-enheter",
    page: "Innstillinger › Lyd",
    state: "Ingen lydenheter funnet",
    recipe: "list_audio_devices:[]",
    boot: {
      fixtures: { ...COLD, list_audio_devices: [] },
      settings: { onboardingDone: true },
      goto: "settings:audio",
    },
    wait: "#settings-audio",
    full: true,
    small: true,
  },
  {
    id: "settings-audio--enheter",
    page: "Innstillinger › Lyd",
    state: "To enheter, mikseren valgt (32 kanaler)",
    recipe: "LIVE",
    boot: { fixtures: LIVE, settings: { ...READY }, goto: "settings:audio" },
    wait: "#device-list",
    full: true,
  },
  {
    id: "settings-audio--kanalrutenett",
    page: "Innstillinger › Lyd",
    state: "Kanalrutenettet for en 32-kanals mikser, med lagret L/R",
    recipe: "deviceChannels",
    boot: {
      fixtures: LIVE,
      settings: {
        ...READY,
        deviceChannels: { [MIXER]: { channelL: 14, channelR: 15 } },
      },
      goto: "settings:audio",
    },
    wait: "#channel-grid-card",
    act: async (page) => {
      await page.locator("#channel-grid-card").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "settings-audio--diagnose",
    page: "Innstillinger › Lyd",
    state: "Lydenhetsdiagnosen (modal) — rader + full systemrapport",
    recipe: "diagnose_audio",
    boot: {
      fixtures: DIAGNOSE,
      settings: { ...READY },
      goto: "settings:audio",
    },
    act: async (page) => {
      await page.locator("#btn-audio-diagnose").click();
      await expect(page.locator("#audio-diagnose-modal")).toBeVisible();
    },
  },
  {
    id: "settings-video--av",
    page: "Innstillinger › Video",
    state: "Video slått av — alt annet skjult",
    recipe: "videoEnabled:false",
    boot: { fixtures: LIVE, settings: { ...READY }, goto: "settings:video" },
    wait: "#settings-video",
    full: true,
    small: true,
  },
  {
    id: "settings-video--pa",
    page: "Innstillinger › Video",
    state: "Video slått på — kameravalg og «behold separat lydfil»",
    recipe: "videoEnabled:true",
    boot: {
      fixtures: LIVE,
      settings: {
        ...READY,
        videoEnabled: true,
        videoDeviceName: "FaceTime HD Camera",
        videoDeviceIndex: 0,
      },
      goto: "settings:video",
    },
    wait: "#video-settings-panel",
    full: true,
  },
  {
    id: "settings-files--standard",
    page: "Innstillinger › Opptak",
    state: "Mappe, filnavn, format, opprydding, stopp ved stillhet, pre-roll",
    recipe: "READY",
    boot: {
      fixtures: LIVE,
      settings: {
        ...READY,
        autoDeleteDays: 90,
        prerollEnabled: true,
        preRollSeconds: 30,
      },
      goto: "settings:files",
    },
    wait: "#settings-files",
    full: true,
    small: true,
  },
  {
    id: "settings-files--stillhet-pa",
    page: "Innstillinger › Opptak",
    state: "«Stopp ved stillhet» på — terskel i dBFS og tidsavbrudd synlig",
    recipe: "stopOnSilence",
    boot: {
      fixtures: LIVE,
      settings: {
        ...READY,
        stopOnSilence: true,
        splitMinutes: 60,
        manualMaxMinutes: 180,
      },
      goto: "settings:files",
    },
    wait: "#silence-config",
    act: async (page) => {
      await page.locator("#silence-config").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "settings-sharing--standard",
    page: "Innstillinger › Deling",
    state: "Varsler + e-post ved feil (alt som er igjen etter Fase R)",
    recipe: "READY",
    boot: { fixtures: LIVE, settings: { ...READY }, goto: "settings:sharing" },
    wait: "#settings-sharing",
    full: true,
    small: true,
  },
  {
    id: "settings-sharing--smtp",
    page: "Innstillinger › Deling",
    state: "E-postvarsel på, SMTP-feltene åpne",
    recipe: "emailOnError",
    boot: {
      fixtures: {
        ...LIVE,
        email_status: { featureBuilt: true },
        email_has_smtp_password: true,
      },
      settings: {
        ...READY,
        emailOnError: true,
        emailAddress: "teknikk@altafrikirke.no",
        emailSmtp: "smtp.domeneshop.no",
        emailSmtpPort: 587,
        emailSmtpUser: "teknikk@altafrikirke.no",
        emailSmtpFrom: "SundayRec <teknikk@altafrikirke.no>",
      },
      goto: "settings:sharing",
    },
    act: async (page) => {
      const adv = page.locator("#email-smtp-advanced");
      if ((await adv.count()) > 0) {
        const summary = adv.locator("summary").first();
        if ((await summary.count()) > 0) await summary.click();
      }
      await expect(page.locator("#email-smtp")).toBeVisible();
    },
    full: true,
  },
  {
    id: "settings-general--standard",
    page: "Innstillinger › System",
    state: "Språk, kirkeprofil, system, oppdatering, logg, diagnostikk",
    recipe: "READY",
    boot: { fixtures: LIVE, settings: { ...READY }, goto: "settings:general" },
    wait: "#settings-general",
    full: true,
    small: true,
  },
  {
    id: "settings-general--telemetri-preview",
    page: "Innstillinger › System",
    state: "«Vis hva som sendes» — hele nyttelasten som JSON",
    recipe: "telemetry_preview_payload",
    boot: {
      fixtures: {
        ...LIVE,
        telemetry_consent_get: {
          status: "granted",
          version: 2,
          decidedAt: 1_754_000_000_000,
          currentVersion: 2,
          needsPrompt: false,
          active: true,
        },
        telemetry_preview_payload: {
          json: JSON.stringify(
            {
              installId: "a1b2c3d4-0000-0000-0000-000000000000",
              app: { version: "0.14.1", os: "macos", arch: "aarch64" },
              counters: [
                { name: "recording_started_scheduled", value: 4 },
                { name: "recording_completed", value: 4 },
              ],
              crashes: [],
            },
            null,
            2,
          ),
          isNextPayload: true,
          isEmpty: false,
        },
      },
      settings: { ...READY },
      goto: "settings:general",
    },
    act: async (page) => {
      await page.locator("#btn-telemetry-preview").click();
      await expect(page.locator("#telemetry-preview-modal")).toBeVisible();
      await expect(page.locator("#telemetry-preview-body")).toContainText(
        "installId",
      );
    },
  },
  {
    id: "settings-general--oppdatering-tilgjengelig",
    page: "Innstillinger › System",
    state: "Oppdateringskortet: en ny versjon finnes",
    recipe: "update_check:available",
    boot: {
      fixtures: {
        ...LIVE,
        update_check: {
          phase: "available",
          version: "0.15.0",
          notes: "Ny hjemskjerm, enklere eksport.",
        },
      },
      settings: { ...READY, autoUpdate: false },
      goto: "settings:general",
    },
    act: async (page) => {
      await page.locator("#btn-check-updates").click();
      await expect(page.locator("#update-status-text")).not.toHaveText("");
      await page.locator("#update-status-text").scrollIntoViewIfNeeded();
    },
  },
  {
    id: "settings-general--oppdatering-klar",
    page: "Innstillinger › System",
    state: "Oppdateringskortet: nedlastet, klar til å installeres",
    recipe: "update_check:downloaded",
    boot: {
      fixtures: {
        ...LIVE,
        update_check: { phase: "downloaded", version: "0.15.0" },
      },
      settings: { ...READY, autoUpdate: false },
      goto: "settings:general",
    },
    act: async (page) => {
      await page.locator("#btn-check-updates").click();
      await expect(page.locator("#update-status-text")).not.toHaveText("");
      await page.locator("#update-status-text").scrollIntoViewIfNeeded();
    },
  },
  {
    id: "settings-general--oppdatering-feil",
    page: "Innstillinger › System",
    state: "Oppdateringskortet: sjekken feilet",
    recipe: "update_check:throws",
    boot: {
      fixtures: {
        ...LIVE,
        update_check: fn('() => { throw new Error("network unreachable") }'),
      },
      settings: { ...READY, autoUpdate: false },
      goto: "settings:general",
    },
    act: async (page) => {
      await page.locator("#btn-check-updates").click();
      await expect(page.locator("#update-status-text")).not.toHaveText("");
      await page.locator("#update-status-text").scrollIntoViewIfNeeded();
    },
  },
  {
    id: "settings-general--oppdatering-varsel",
    page: "Innstillinger › System",
    state: "Oppdateringsvarselet i sidepanelet (update-toast)",
    recipe: "update-toast",
    boot: {
      fixtures: {
        ...LIVE,
        update_check: {
          phase: "available",
          version: "0.15.0",
          notes: "Ny hjemskjerm, enklere eksport.",
        },
      },
      settings: { ...READY, autoUpdate: true },
      goto: "home",
    },
    act: async (page) => {
      const toast = page.locator("#update-toast");
      await expect(toast).toBeVisible({ timeout: 15_000 });
    },
  },

  // ══ HISTORIKK / SØK ════════════════════════════════════════════════════════
  {
    id: "search--tom",
    page: "Historikk",
    state: "Ingen opptak ennå",
    recipe: "recordings_list:[]",
    boot: {
      fixtures: COLD,
      settings: { onboardingDone: true },
      goto: "search",
    },
    wait: "#page-search",
    full: true,
    small: true,
  },
  {
    id: "search--med-opptak",
    page: "Historikk",
    state: "Fem opptak: langt, kort, video, med notat, langt filnavn",
    recipe: "recordings_list",
    boot: { fixtures: LIVE, settings: { ...READY }, goto: "search" },
    wait: "#history-tbody tr.hist-row",
    full: true,
  },
  {
    id: "search--treff",
    page: "Historikk",
    state: "Søk på «bønne» — filtrert liste, statistikken følger filteret",
    recipe: "#search-query",
    boot: { fixtures: LIVE, settings: { ...READY }, goto: "search" },
    act: async (page) => {
      await page.locator("#search-query").fill("bønne");
      await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(1);
    },
  },
  {
    id: "search--ingen-treff",
    page: "Historikk",
    state: "Søk uten treff — egen melding, ikke «ingen opptak ennå»",
    recipe: "#search-query",
    boot: { fixtures: LIVE, settings: { ...READY }, goto: "search" },
    act: async (page) => {
      await page.locator("#search-query").fill("julaften");
      await expect(page.locator("#search-index-status")).toContainText(
        /Ingen treff for|No matches for/,
      );
    },
  },
  {
    id: "search--flere-verktoy",
    page: "Historikk",
    state: "«Flere»-panelet: slett feilede, rydd historikk",
    recipe: "#btn-history-more",
    boot: { fixtures: LIVE, settings: { ...READY }, goto: "search" },
    act: async (page) => {
      await page.locator("#btn-history-more").click();
      await expect(page.locator("#history-more-panel")).toBeVisible();
    },
  },
  // NB: there is deliberately no `search--papirkurv-tom` scene. `refreshTrashButton()`
  // in pages/history.ts hides `#btn-trash-open` outright when the trash is empty
  // («An empty trash is not a place worth offering to visit») — so the empty
  // trash view has no entry point and cannot be photographed. Recorded as a
  // finding in ../../docs/design/ATLAS.md §5 rather than faked here.
  {
    id: "search--papirkurv-fylt",
    page: "Historikk › Papirkurv",
    state: "Ett slettet opptak, med «tøm papirkurv»",
    recipe: "trash_list",
    boot: {
      fixtures: { ...LIVE, trash_list: TRASHED },
      settings: { ...READY },
      goto: "search",
    },
    act: async (page) => {
      await page.locator("#btn-trash-open").click();
      await expect(page.locator("#trash-view")).toBeVisible();
    },
  },
  {
    id: "search--notat-dialog",
    page: "Historikk",
    state: "Notat-dialogen på en rad",
    recipe: "modal-note",
    boot: {
      fixtures: { ...LIVE, recording_update_note: true },
      settings: { ...READY },
      goto: "search",
    },
    act: async (page) => {
      // The note icon carries no id and its `title` is localized. It IS the last
      // non-delete action on the row (history.ts appends reveal → edit →
      // [reveal video] → note → delete), so pick it by position rather than by
      // a string that changes with the locale. Clicking the others would open
      // the editor or hit a native reveal instead.
      const row = page.locator("#history-tbody tr.hist-row").first();
      await row.locator("a.hist-action:not(.hist-del)").last().click();
      await expect(page.locator("#modal-note")).toBeVisible();
    },
  },
  {
    id: "search--slett-angre",
    page: "Historikk",
    state: "Sletting: ingen bekreftelse, men en «Angre»-toast (suksess-toast)",
    recipe: "trash_move",
    boot: {
      fixtures: {
        ...LIVE,
        trash_move: fn(`(args) => args.paths.map((p, i) => ({
          id: "t" + i, originalPath: p, trashedPath: "/tmp/trash/x",
          name: p.split("/").pop(), deletedAt: 1787000000000, related: [], byteSize: 1000,
        }))`),
      },
      settings: { ...READY },
      goto: "search",
    },
    act: async (page) => {
      await page
        .locator("#history-tbody tr.hist-row")
        .first()
        .locator("a.hist-del")
        .click();
      await expect(page.locator(".ui-toast")).toBeVisible();
    },
  },

  // ══ REDIGER ════════════════════════════════════════════════════════════════
  {
    id: "editor--tom",
    page: "Rediger",
    state: "Ingen fil åpen — slippsone og siste opptak",
    recipe: "goto=editor",
    boot: { fixtures: LIVE, settings: { ...READY }, goto: "editor" },
    wait: "#editor-empty",
    full: true,
    small: true,
  },
  {
    id: "editor--laster",
    page: "Rediger",
    state: "«Analyserer …» — fila leses og bølgeformen bygges",
    recipe: "editor_load_recording:pending",
    boot: {
      fixtures: editorFixtures({
        editor_load_recording: fn("() => new Promise(() => {})"),
      }),
      settings: { ...READY },
      goto: "editor",
    },
    act: async (page) => {
      await page.evaluate(
        (f) =>
          (
            window as unknown as { openEditorWithFile: (p: string) => void }
          ).openEditorWithFile(f),
        EDITOR_FILE,
      );
      await expect(page.locator("#editor-loading")).toBeVisible();
    },
  },
  {
    id: "editor--feil",
    page: "Rediger",
    state: "Fila kunne ikke leses",
    recipe: "editor_load_recording:throws",
    boot: {
      fixtures: editorFixtures({
        editor_load_recording: fn(
          '() => { throw new Error("unsupported codec") }',
        ),
      }),
      settings: { ...READY },
      goto: "editor",
    },
    act: async (page) => {
      await page.evaluate(
        (f) =>
          (
            window as unknown as { openEditorWithFile: (p: string) => void }
          ).openEditorWithFile(f),
        EDITOR_FILE,
      );
      await expect(page.locator("#editor-loading")).toBeHidden({
        timeout: 20_000,
      });
    },
  },
  {
    id: "editor--lyd-fane",
    page: "Rediger › Lyd",
    state: "Åpnet opptak: bølgeform, normalisering, intro/outro, mastering",
    recipe: "editorFixtures",
    boot: {
      fixtures: editorFixtures(),
      settings: { ...READY },
      goto: "editor",
    },
    act: openEditor,
    full: true,
    small: true,
  },
  {
    id: "editor--innhold-fane",
    page: "Rediger › Innhold",
    state: "Metadata: tittel, taler, beskrivelse",
    recipe: "editorFixtures",
    boot: {
      fixtures: editorFixtures(),
      settings: { ...READY },
      goto: "editor",
    },
    act: async (page) => {
      await openEditor(page);
      await page.locator("#editor-tab-content").click();
      await expect(page.locator("#editor-tabpanel-content")).toBeVisible();
    },
    full: true,
  },
  {
    id: "editor--klipp-fane",
    page: "Rediger › Klipp",
    state: "Segmenter funnet, prekenvelger, «Marker preken automatisk»",
    recipe: "editor_segments",
    boot: {
      fixtures: editorFixtures(),
      settings: { ...READY },
      goto: "editor",
    },
    act: async (page) => {
      await openEditor(page);
      await page.locator("#editor-tab-clip").click();
      await expect(page.locator("#editor-tabpanel-clip")).toBeVisible();
    },
    full: true,
  },
  {
    id: "editor--kuttliste",
    page: "Rediger › Klipp",
    state: "Etter «Marker preken automatisk»: to kutt i kuttlisten",
    recipe: "#btn-apply-auto-trim",
    boot: {
      fixtures: editorFixtures(),
      settings: { ...READY },
      goto: "editor",
    },
    act: async (page) => {
      await openEditor(page);
      await page.locator("#editor-tab-clip").click();
      await page.locator("#btn-apply-auto-trim").click();
      await expect(
        page.locator("#editor-cuts-list .editor-cut-row").first(),
      ).toBeVisible();
    },
    full: true,
  },
  {
    id: "editor--mastering-panel",
    page: "Rediger › Lyd",
    state:
      "Mastering-panelet utvidet (ett av fem steder mastring finnes — ATLAS.md §3c)",
    recipe: "editor_master_presets",
    boot: {
      fixtures: editorFixtures(),
      settings: { ...READY },
      goto: "editor",
    },
    act: async (page) => {
      await openEditor(page);
      await page.locator("#editor-master-header").click();
      await expect(page.locator("#master-preset-select")).toBeVisible();
      await page.locator("#editor-master-section").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "editor--eksport-modal",
    page: "Rediger",
    state:
      "Eksportmodalen for lyd: format, bitrate, destinasjon, lydforbedring",
    recipe: "#btn-editor-save",
    boot: {
      fixtures: editorFixtures(),
      settings: { ...READY },
      goto: "editor",
    },
    act: async (page) => {
      await openEditor(page);
      await page.locator("#btn-editor-save").click();
      await expect(page.locator("#editor-export-modal")).toBeVisible();
    },
    full: true,
  },
  {
    id: "editor--eksport-modal-video",
    page: "Rediger",
    state: "Eksportmodalen for et videoopptak: eksporttype, kodek, format",
    recipe: "hasVideo:true",
    boot: {
      fixtures: editorFixtures({
        editor_probe_streams: { hasVideo: true, hasAudio: true },
        editor_load_recording: {
          durationSec: EDITOR_DURATION,
          hasVideo: true,
          hasAudio: true,
          channels: 2,
          sampleFmt: "s16",
          sampleRate: 48_000,
        },
      }),
      settings: { ...READY },
      goto: "editor",
    },
    act: async (page) => {
      await openEditor(page);
      await page.locator("#btn-editor-save").click();
      await expect(page.locator("#editor-export-modal")).toBeVisible();
    },
    full: true,
  },

  // ══ FØRSTE OPPSTART ════════════════════════════════════════════════════════
  ...onboardingScenes(),

  // ══ TOASTS ═════════════════════════════════════════════════════════════════
  {
    id: "toast--lagring-feilet",
    page: "Innstillinger › System",
    state: "Feil-toast: innstillingen kunne ikke lagres",
    recipe: "settings_save:throws",
    boot: {
      fixtures: {
        ...LIVE,
        settings_save: fn('() => { throw new Error("database is locked") }'),
      },
      settings: { ...READY },
      goto: "settings:general",
    },
    act: async (page) => {
      await page
        .locator("label.toggle:has(#opt-ask-open-editor) .toggle-track")
        .click();
      await expect(page.locator(".ui-toast")).toBeVisible();
    },
  },
];

/** The first-run wizard, one scene per step. Must boot WITHOUT `?goto=` —
 *  `loadSettings` forces `onboardingDone` whenever that param is present. */
function onboardingScenes(): Scene[] {
  const base: Fixtures = {
    ...LIVE,
    telemetry_consent_set: {
      status: "denied",
      version: 2,
      decidedAt: 1_787_000_000_000,
      currentVersion: 2,
      needsPrompt: false,
      active: false,
    },
  };
  const steps: Array<{
    n: number;
    slug: string;
    state: string;
    step: (p: Page) => Promise<void>;
  }> = [
    {
      n: 1,
      slug: "1-velkommen",
      state: "Steg 1 — velkommen",
      step: async () => undefined,
    },
    {
      n: 2,
      slug: "2-lydenhet",
      state: "Steg 2 — hvilken lydenhet bruker dere?",
      step: async (p) => {
        await p.locator("#ob-n1").click();
      },
    },
    {
      n: 3,
      slug: "3-lydtest",
      state: "Steg 3 — test at lyden fungerer (lydtest-porten)",
      step: async (p) => {
        await p.locator("#ob-n1").click();
        await p.locator("#ob-s2").click();
      },
    },
    {
      n: 4,
      slug: "4-tidsplan",
      state: "Steg 4 — ukentlig automatisk opptak",
      step: async (p) => {
        await p.locator("#ob-n1").click();
        await p.locator("#ob-s2").click();
        await p.locator("#ob-s3").click();
      },
    },
    {
      n: 5,
      slug: "5-samtykke",
      state: "Steg 5 — vil du hjelpe oss? (diagnostikk-samtykke)",
      step: async (p) => {
        await p.locator("#ob-n1").click();
        await p.locator("#ob-s2").click();
        await p.locator("#ob-s3").click();
        await p.locator("#ob-s4").click();
      },
    },
    {
      n: 6,
      slug: "6-ferdig",
      state: "Steg 6 — alt er klart",
      step: async (p) => {
        await p.locator("#ob-n1").click();
        await p.locator("#ob-s2").click();
        await p.locator("#ob-s3").click();
        await p.locator("#ob-s4").click();
        await p.locator("#ob-consent-no").click();
      },
    },
  ];

  return steps.map((s) => ({
    id: `onboarding--${s.slug}`,
    page: "Første oppstart",
    state: s.state,
    recipe: "onboardingDone:false",
    boot: { fixtures: base, settings: { onboardingDone: false } },
    act: async (page: Page) => {
      await expect(page.locator("#onboarding-overlay")).toBeVisible();
      await s.step(page);
      // `.ob-title` alone: `#ob-body` also carries the class `ob-body`, so the
      // pair `.ob-title, .ob-body` is a strict-mode violation on every step that
      // has both.
      await expect(page.locator(".ob-title").first()).toBeVisible();
    },
    small: s.n === 1,
  }));
}

/** Every scene id, for the console report and INDEX. */
export const SCENE_IDS = SCENES.map((s) => s.id);
