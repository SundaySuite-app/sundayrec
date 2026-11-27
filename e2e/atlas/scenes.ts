import { expect, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  BOOT_FIXTURES,
  fn,
  recordingRow,
  SETTLED_SETTINGS,
  VOID,
  type BootOptions,
  type Fixtures,
} from "../harness";
import type { WakeCapabilities } from "@legacy/bindings/WakeCapabilities";
import {
  editorFixtures,
  EXPORT_HELD,
  FILE,
  RECORDING_ID,
} from "../editor-fixtures";
import {
  advanceClock,
  ATLAS_NOW,
  emitAt,
  emitEventAt,
  LAST_SUNDAY_MS,
  NEXT_SERVICE_ISO,
  OLD_SUNDAY_MS,
  PREV_SUNDAY_MS,
  settleVu,
} from "./harness";
import { cameraFixtures, FRAME_16_9, pushFrame } from "../camera-feed";

/**
 * THE SCENE TABLE — every screen and state the atlas photographs.
 *
 * Written against `data-testid`, and only against `data-testid`. The fase A
 * table was written against the legacy shell's element ids (`#page-search`,
 * `#editor-tab-clip`), and when that shell was deleted every single scene died
 * with it. Testids are the contract the rest of `e2e/` already holds the app
 * to, so a scene that breaks here breaks for the same reason a spec would.
 *
 * ## The four levers a scene has
 *
 * `boot.fixtures` — canned answers per Tauri command, through the api-shim
 * seam. Command names are the RUST names (snake_case); their payloads are
 * camelCase, except `recordings_list`, which answers snake_case rows.
 * `boot.settings` — a partial settings object merged over the defaults.
 * `boot.goto` — a deep link in the F5 vocabulary (`record`, `recordings`, `plan`, `settings`).
 * `act` drives clicks and backend events; `pre` installs anything that has to
 * exist before the renderer's first line runs.
 *
 * ## What is NOT here, and why
 *
 * Five states are known-unphotographable in a browser, and it is better to say
 * so once than to leave a reader hunting for them:
 *
 *  - **The `rec` status sentence in the bottom bar.** It needs `isRecording`,
 *    and `isRecording` also paints the recording overlay, which is
 *    `position: fixed; inset: 0` on purpose — while a recording runs, the
 *    overlay IS the app. The sentence exists; the pixel cannot.
 *    `opptaksmodus--*` is what a recording actually looks like.
 *  - **The hold in progress.** The red «Hold» ring fills over one second of
 *    CSS transition; a photograph of «halfway» would be a photograph of the
 *    shutter, not of a state. The ring at rest, and the dialog that screen
 *    readers get instead, are photographed.
 *  - **The ASIO attribution card** (Om › ASIO-kreditering): it gates on
 *    `currentOs() === "win"`, and `navigator.userAgentData.platform` beats any
 *    user-agent override, so Desktop Chrome can never be Windows here.
 *  - **Native pickers** («Åpne fil…», «Velg mappe», intro/outro «+ Velg fil»).
 *    All of them are opened by Rust itself, which no fixture answers. The state
 *    BEFORE the click is photographed; the state after does not exist out here.
 *  - **Video in the editor.** `asset://` is dead outside Tauri and Chromium has
 *    no H.264, so the editor's `<video>` frame is always black here (the frame,
 *    its transport and the waveform under it are what the scene shows). The same
 *    goes for playback: the play button is `aria-disabled`.
 */

/** One photograph: what to boot, how to get there, and what it is called. */
export interface Scene {
  /** File name stem, and the row's id in INDEX.md. */
  id: string;
  /** Which destination it belongs to — INDEX.md groups on this. */
  page: string;
  /** What state of that destination this is. */
  state: string;
  /** One line of "how", for a reader of INDEX.md who will not open the code. */
  recipe: string;
  boot: BootOptions;
  /** Runs BEFORE `boot` — init scripts, stubs. */
  pre?: (page: Page) => Promise<void>;
  /** A `data-testid` that must be visible before anything else happens. */
  wait?: string;
  /** Drive the scene the rest of the way. */
  act?: (page: Page) => Promise<void>;
  /** Also grow the viewport to the content height when the page scrolls. */
  full?: boolean;
  /** Also photograph at 1000×760 (one column). */
  narrow?: boolean;
}

// ── Shared fixtures ─────────────────────────────────────────────────────────

/** The version the shipped app would show. Read, never typed out. */
const PKG_VERSION: string = JSON.parse(
  readFileSync(
    join(dirname(fileURLToPath(import.meta.url)), "../../package.json"),
    "utf8",
  ),
).version;

/**
 * The base every scene starts from.
 *
 * `update_check` is the one addition worth explaining: `autoUpdate` defaults to
 * TRUE, and `BOOT_FIXTURES` answers no `update_check`, so the boot-time check
 * rejects and the update row paints its red «Kunne ikke sjekke etter
 * oppdateringer» line — on every single photograph of Innstillinger. An atlas
 * where every picture carried an error nobody actually has would be a worse lie
 * than one that shows a healthy machine, so the healthy machine is the baseline
 * and the failure gets a scene of its own.
 */
const ATLAS_ONLY: Fixtures = {
  // The version in the bottom bar is in every single photograph, so it must not
  // be the browser tier's `0.10.0-e2e` placeholder — an atlas of v0.17.0 that
  // says 0.10.0 in the corner is a document that argues with itself. Read from
  // package.json rather than written down, so it can never go stale.
  app_info: { version: PKG_VERSION },
  update_check: { phase: "upToDate" },
  // `list_devices` er den ekte kameralista (shimmens `listVideoDevices` leser
  // `video_inputs` ut av den). En `list_video_devices`-nøkkel sto ved siden av
  // og styrte ingenting — kommandoen er slettet i V1-halen.
  list_devices: { video_inputs: [] },
};

const BASE: Fixtures = { ...BOOT_FIXTURES, ...ATLAS_ONLY };

/** One audio device, in `list_audio_devices`' shape. */
function device(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: "x32",
    name: "Behringer X32",
    backend: "coreaudio",
    inputChannels: 2,
    sampleRates: [48000],
    isDefault: false,
    ...over,
  };
}

const BUILT_IN = device({
  id: "builtin",
  name: "MacBook Pro-mikrofon",
  isDefault: true,
});

/** The devices a healthy machine offers. */
const DEVICES: Fixtures = { list_audio_devices: [device(), BUILT_IN] };

/** Settings where the source question has been answered. */
const CHOSEN = {
  ...SETTLED_SETTINGS,
  deviceId: "x32",
  deviceName: "Behringer X32",
  saveFolder: "/Users/frivillig/Opptak",
};

/** A camera that is on, and the backend list that agrees it exists. */
const WITH_CAMERA = {
  ...CHOSEN,
  videoEnabled: true,
  videoDeviceName: "Logitech BRIO",
  videoDeviceIndex: 0,
};
const CAMERA_FIXTURES: Fixtures = {
  ...BASE,
  ...DEVICES,
  list_devices: { video_inputs: [{ name: "Logitech BRIO", index: 0 }] },
};

/** The library rows the atlas keeps showing. FIXED timestamps, so «Søndag 16.
 *  august · 11:00» is the same string on every run. */
const LIBRARY_ROWS = [
  recordingRow({
    id: "rec-a",
    file_path: "/Users/frivillig/Opptak/2026-08-16 Gudstjeneste.mp3",
    device_name: "Behringer X32",
    started_at: LAST_SUNDAY_MS,
    created_at: LAST_SUNDAY_MS,
    duration_ms: 4_920_000,
    byte_size: 118_000_000,
    note: "Dåp — Ida leste teksten",
  }),
  recordingRow({
    id: "rec-b",
    file_path: "/Users/frivillig/Opptak/2026-08-09 Gudstjeneste.mp3",
    device_name: "Behringer X32",
    started_at: PREV_SUNDAY_MS,
    created_at: PREV_SUNDAY_MS,
    duration_ms: 3_660_000,
    byte_size: 88_000_000,
  }),
  recordingRow({
    id: "rec-c",
    file_path: "/Users/frivillig/Opptak/2026-08-05 Bønnemøte.mp3",
    device_name: "MacBook Pro-mikrofon",
    started_at: Date.parse("2026-08-05T19:00:00+02:00"),
    created_at: Date.parse("2026-08-05T19:00:00+02:00"),
    duration_ms: 1_500_000,
    byte_size: 36_000_000,
  }),
];

/** One trash entry. `deletedAt` counts from the FROZEN clock, never from the
 *  runner's — otherwise «Slettes om N dager» drifts by a day at midnight. */
function trashEntry(
  over: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    id: "t1",
    originalPath: "/Users/frivillig/Opptak/2026-07-26 Gudstjeneste.mp3",
    trashedPath: "/Users/frivillig/Opptak/.sundayrec-trash/t1.mp3",
    name: "2026-07-26 Gudstjeneste.mp3",
    deletedAt: OLD_SUNDAY_MS,
    related: [],
    byteSize: 86_000_000,
    ...over,
  };
}

// ── The editor ──────────────────────────────────────────────────────────────
//
// `e2e/editor-fixtures.ts` is the browser tier's own set, imported by every
// editor spec. The atlas uses it unchanged — a second copy that drifted would
// photograph an editor nothing else tests.

/**
 * The editor's own fixtures, with the atlas's on top.
 *
 * ⚠️ The order is the whole point, and getting it wrong is silent.
 * `editorFixtures()` spreads `BOOT_FIXTURES` ITSELF and then the caller's
 * overrides, so `{ ...BASE, ...editorFixtures() }` puts the browser tier's
 * defaults back on top of the atlas's: every editor and export photograph came
 * out with an empty device list («Lyden er ikke koblet til» in the status line)
 * and `0.10.0-e2e` in the corner. Feeding them THROUGH `editorFixtures` is what
 * makes them win.
 */
function editorScene(over: Fixtures = {}): Fixtures {
  return editorFixtures({ ...DEVICES, ...ATLAS_ONLY, ...over });
}

/** Open a file through the same global the editor specs use. */
async function openFile(page: Page, recordingId = RECORDING_ID): Promise<void> {
  await page.evaluate(
    (f) =>
      (
        window as unknown as { openEditorWithRecording: (id: string) => void }
      ).openEditorWithRecording(f),
    recordingId,
  );
}

/** Open the fixtured recording and wait until the workspace is actually there. */
async function openEditor(page: Page): Promise<void> {
  await openFile(page);
  await expect(page.getByTestId("editor")).toHaveAttribute(
    "data-state",
    "ready",
  );
}

/** The commands a healthy machine answers (devices, permissions, ffmpeg, wake). */
const HEALTHY: Fixtures = {
  ...BASE,
  ...DEVICES,
  media_permissions: { camera: "authorized", microphone: "authorized" },
  ffmpeg_health: { available: true, version: "ffmpeg version 7.1", path: "/x" },
  // F2-I18N-R2: `satisfies` er poenget. `platform` sto på `"macos"` — ikke en
  // verdi `WakePlatform` har — og ingenting merket det, fordi `Fixtures` er
  // `Record<string, unknown>`. Nå er fixturen bundet til bindingen, så en
  // omdøpt kode felles av `tsc` i stedet for av et tomt punkt i atlaset.
  wake_capabilities: {
    platform: "mac-arm",
    canWakeFromSleep: true,
    canWakeFromOff: false,
    needsAdmin: true,
    knownIssues: ["macArmNoPowerOn"],
    recommendations: ["macKeepAsleep", "macDisableStandby", "macStayPluggedIn"],
  } satisfies WakeCapabilities,
};

/** Innstillinger is the same surface every time; only the settings differ. */
function settingsScene(
  settings: Record<string, unknown>,
  fixtures: Fixtures = HEALTHY,
): BootOptions {
  return { fixtures, settings, goto: "settings" };
}

// ── First run ───────────────────────────────────────────────────────────────
//
// ⚠️ NEVER with `?goto=`: the shim forces `onboardingDone = true` whenever the
// parameter is present, so a deep-linked boot can never see the sequence.

const FIRST_RUN_FIXTURES: Fixtures = {
  ...BASE,
  ...DEVICES,
  start_vu: 2,
  stop_vu: VOID,
};

const FIRST_RUN_SETTINGS = {
  onboardingDone: false,
  deviceId: "x32",
  deviceName: "Behringer X32",
};

/** Step 1 is gated on hearing something. «Fortsett uten lyd» is the
 *  deterministic way past it, and it keeps the gate open for the rest. */
async function skipGate(page: Page): Promise<void> {
  await page.getByTestId("hint-skip-sound").click();
}

/** Advance N times from wherever the sequence currently stands. */
async function nextSteps(page: Page, n: number): Promise<void> {
  for (let i = 0; i < n; i += 1) {
    await page.getByTestId("hint-next").click();
  }
}

// ── The table ───────────────────────────────────────────────────────────────

/** A week with three fixed times, one of them missed. */
const THREE_SLOTS = [
  { days: [6], start: "11:00", stop: "12:30", max: null },
  { days: [2], start: "19:00", stop: "20:30", max: null },
  { days: [4], start: "09:30", stop: "10:30", max: null },
];

/** One Sunday service — enough for a strip with a «Neste». */
const SUNDAY_SLOT = [{ days: [6], start: "11:00", stop: "12:30", max: null }];

/** Peaks with two long silences (100–160 s and 300–330 s) for the suggestions. */
const SILENT_PEAKS = fn(`() => ({
  peaks: Array.from({ length: 600 * 100 }, (_, i) => {
    const s = i / 100;
    return (s >= 100 && s < 160) || (s >= 300 && s < 330) ? 0 : 0.6;
  }),
  sampleRate: 8000,
})`);

/** A recording that has a picture. */
const WITH_VIDEO_TRACK = {
  editor_load_recording: {
    durationSec: 600,
    hasVideo: true,
    hasAudio: true,
    channels: 2,
    sampleFmt: "s16",
    sampleRate: 48_000,
  },
};

/** Start the recording overlay the way the planner would, and let it settle. */
async function startOverlay(
  page: Page,
  over: Record<string, unknown> = {},
): Promise<void> {
  await emitAt(page, "recording-overlay-stop", {
    state: "recording",
    scheduled_stop_ms: null,
    ...over,
  });
  await expect(page.getByTestId("recording-overlay")).toBeVisible();
  // Klokka står stille, så en teller som aldri hadde flyttet seg ville stått
  // på 00:00:00 i hver eneste kjøring. Å flytte den FASTE tiden gir et tall
  // som både er ekte og likt neste gang.
  await advanceClock(page, 42);
  await expect(page.getByTestId("overlay-timer")).toContainText("42:0");
  await settleVu(page, "recording-levels", -14, -6);
}

/** The camera, live, before anything else happens. */
async function liveCamera(page: Page, testId: string): Promise<void> {
  await pushFrame(page, FRAME_16_9);
  await expect(page.getByTestId(testId)).toHaveAttribute("data-phase", "live");
}

export const SCENES: Scene[] = [
  // ══ TA OPP — hvile ════════════════════════════════════════════════════════
  {
    id: "opptak--kald",
    page: "Ta opp",
    state: "Kald start — ingen lydkilde valgt ennå",
    recipe: "ingen `deviceId`; den store knappen er sperret og sier hvorfor",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: SETTLED_SETTINGS,
      goto: "record",
    },
    wait: "record-no-source",
    narrow: true,
  },
  {
    id: "opptak--klar",
    page: "Ta opp",
    state:
      "Klar — kilde-brikka med L/R-måler, den store knappen, og ukestripa med «Neste»",
    recipe:
      "`deviceId:x32` + én fast tid + `scheduler_status.next`; måleren settles",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        scheduler_status: { next: NEXT_SERVICE_ISO },
        recordings_list: LIBRARY_ROWS,
      },
      settings: {
        ...CHOSEN,
        autoRecordEnabled: true,
        slots: SUNDAY_SLOT,
      },
      goto: "record",
    },
    wait: "record-start",
    act: async (page) => {
      await expect(page.getByTestId("record-next-auto")).toBeVisible();
      await settleVu(page);
    },
    narrow: true,
  },
  {
    id: "opptak--kilde-borte",
    page: "Ta opp",
    state: "Kilden er valgt, men ikke til stede nå",
    recipe: "`list_audio_devices` uten `x32`; Start er ÅPEN, med advarsel",
    boot: {
      fixtures: { ...BASE, list_audio_devices: [BUILT_IN] },
      settings: CHOSEN,
      goto: "record",
    },
    wait: "record-source-missing",
  },

  // ── Kameraet: bildet fyller flaten ────────────────────────────────────────
  {
    id: "opptak--kamera-live",
    page: "Ta opp",
    state:
      "Kamera på — bildet fyller flaten, med kameranavn, måler i kanten og liten opptaksknapp",
    recipe:
      "`videoEnabled:true` + `camera_preview_start`-fixtur og en ekte JPEG ned kanalen",
    boot: {
      fixtures: { ...CAMERA_FIXTURES, ...cameraFixtures() },
      settings: WITH_CAMERA,
      goto: "record",
    },
    wait: "record-camera-preview",
    act: async (page) => {
      // Vent til rammen SIER at bildet er der. Aldri en assertion på piksler:
      // `data-phase` er appens egen påstand, og det er den som kan være feil.
      await liveCamera(page, "record-camera-preview");
      await settleVu(page);
    },
  },
  {
    id: "opptak--kamera-valg",
    page: "Ta opp",
    state: "Kamera på — popoveren bak den lille pila: kamera og egen lydfil",
    recipe: "som over, så klikk `record-camera-more`",
    boot: {
      fixtures: { ...CAMERA_FIXTURES, ...cameraFixtures() },
      settings: WITH_CAMERA,
      goto: "record",
    },
    wait: "record-camera-preview",
    act: async (page) => {
      await liveCamera(page, "record-camera-preview");
      await settleVu(page);
      await page.getByTestId("record-camera-more").click();
      await expect(page.getByTestId("record-camera-popover")).toBeVisible();
    },
  },
  {
    id: "opptak--kamera-nektet",
    page: "Ta opp",
    state: "Kamerabildet — fasen `denied` (OS-et sa nei)",
    recipe: "`camera_preview_start` som svarer «camera access is not allowed»",
    boot: {
      fixtures: { ...CAMERA_FIXTURES, ...cameraFixtures("denied") },
      settings: WITH_CAMERA,
      goto: "record",
    },
    wait: "record-camera-preview",
    act: async (page) => {
      await expect(page.getByTestId("record-camera-preview")).toHaveAttribute(
        "data-phase",
        "denied",
      );
      await settleVu(page);
    },
  },
  {
    id: "opptak--kamera-borte",
    page: "Ta opp",
    state: "Kamerabildet — det lagrede kameraet finnes ikke",
    recipe: "`videoDeviceName` som ikke er i bakendens kameraliste",
    boot: {
      fixtures: CAMERA_FIXTURES,
      settings: { ...WITH_CAMERA, videoDeviceName: "Sony HandyCam" },
      goto: "record",
    },
    wait: "record-camera-preview",
    act: async (page) => {
      await expect(
        page.getByTestId("record-camera-preview-message"),
      ).toBeVisible();
      await settleVu(page);
    },
  },

  // ══ LYDARKET ══════════════════════════════════════════════════════════════
  {
    id: "lydark--enhet",
    page: "Lydarket",
    state: "Lydarket — enhetskort, kanalpar og stor L/R-måler",
    recipe: "`?goto=settings:sound` → `record` + panelet `source`",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: CHOSEN,
      goto: "settings:sound",
    },
    wait: "setup-sound",
    act: async (page) => {
      await expect(page.getByTestId("source-sheet")).toHaveAttribute(
        "data-open",
        "true",
      );
      await settleVu(page);
    },
  },
  {
    id: "lydark--mer",
    page: "Lydarket",
    state:
      "«Mer» utfoldet — mappe, kvalitet og de fem flisene (Før, Stille, Del, Rydd, Maks)",
    recipe:
      "`?goto=settings:engine` → panelet `source-more`; alle fem flisene på",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: {
        ...CHOSEN,
        preRollSeconds: 15,
        stopOnSilence: true,
        silenceTimeoutMinutes: 5,
        splitMinutes: 60,
        autoDeleteDays: 90,
        manualMaxMinutes: 120,
      },
      goto: "settings:engine",
    },
    wait: "tile-max",
    act: async (page) => {
      await expect(page.getByTestId("setup-folder")).toBeVisible();
      await settleVu(page);
    },
    full: true,
  },
  {
    id: "lydark--mer-flisene-av",
    page: "Lydarket",
    state: "«Mer» utfoldet — alle fem flisene av, «Av» står under dem",
    recipe: "som over, med alle fem verdiene på 0",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: {
        ...CHOSEN,
        preRollSeconds: 0,
        stopOnSilence: false,
        splitMinutes: 0,
        autoDeleteDays: 0,
        manualMaxMinutes: 0,
      },
      goto: "settings:engine",
    },
    wait: "tile-max",
    act: async (page) => {
      await settleVu(page);
    },
    full: true,
  },

  // ══ UKESTRIPA ═════════════════════════════════════════════════════════════
  {
    id: "strip--pille",
    page: "Ukestripa",
    state:
      "Uten faste tider: bare den lille kalenderknappen nede til venstre, med gullplussen",
    recipe: "`slots: []`",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: { ...CHOSEN, slots: [] },
      goto: "record",
    },
    wait: "record-next-none",
    act: async (page) => {
      await settleVu(page);
    },
  },
  {
    id: "strip--lukket",
    page: "Ukestripa",
    state:
      "Lukket stripe — mini-uke, «Neste: søndag 11:00», nedtelling og de fire statusikonene",
    recipe: "én fast tid + `scheduler_status.next`, Auto på",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        scheduler_status: { next: NEXT_SERVICE_ISO },
      },
      settings: { ...CHOSEN, autoRecordEnabled: true, slots: THREE_SLOTS },
      goto: "record",
    },
    wait: "record-next-auto",
    act: async (page) => {
      await settleVu(page);
    },
  },
  {
    id: "strip--lukket-auto-av",
    page: "Ukestripa",
    state:
      "Lukket stripe med tider men Auto AV — dempet, «Automatisk opptak er av»",
    recipe: "`slots` satt, `autoRecordEnabled:false`",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: { ...CHOSEN, autoRecordEnabled: false, slots: THREE_SLOTS },
      goto: "record",
    },
    wait: "record-next-none",
    act: async (page) => {
      await expect(page.getByTestId("record-next-auto-when")).toBeVisible();
      await settleVu(page);
    },
  },
  {
    id: "strip--apen",
    page: "Ukestripa",
    state:
      "Åpen stripe — de fire flisene og ukevisningen, «Neste» fremhevet, ett opptak denne uka ikke tatt opp",
    recipe:
      "`?goto=plan`; tre tider, `scheduler_status.next` på søndagen, `emitEvent('scheduler://missed')`",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        scheduler_status: { next: NEXT_SERVICE_ISO },
      },
      settings: { ...CHOSEN, autoRecordEnabled: true, slots: THREE_SLOTS },
      goto: "plan",
    },
    wait: "plan-week",
    act: async (page) => {
      // ATLAS_NOW er søndag 23. august 2026: fredagen i samme uke er 21.
      await emitEventAt(page, "scheduler://missed", [
        { at: "2026-08-21T09:30:00", label: "Bibeltime" },
      ]);
      await expect(
        page.locator('[data-testid="plan-block"][data-missed="true"]'),
      ).toHaveCount(1);
      await settleVu(page);
    },
  },
  {
    id: "strip--blokk",
    page: "Ukestripa",
    state: "Åpen stripe — redigeringen åpen ved siden av en valgt blokk",
    recipe: "som over, så et trykk på søndagsblokka",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        scheduler_status: { next: NEXT_SERVICE_ISO },
      },
      settings: {
        ...CHOSEN,
        autoRecordEnabled: true,
        slots: THREE_SLOTS.slice(0, 2),
      },
      goto: "plan",
    },
    wait: "plan-week",
    act: async (page) => {
      // Første trykk velger blokka, trykk på den valgte åpner redigeringen.
      const block = page.locator('[data-testid="plan-block"][data-day="6"]');
      await block.click();
      await expect(block).toHaveAttribute("data-selected", "true");
      await block.click();
      await expect(page.getByTestId("plan-editor")).toBeVisible();
    },
  },
  {
    id: "strip--tom-uke",
    page: "Ukestripa",
    state: "Åpen stripe med tom uke — «Hold og dra», ingen faste tider ennå",
    recipe: "`?goto=plan`, `slots: []`",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: { ...CHOSEN, slots: [] },
      goto: "plan",
    },
    wait: "plan-empty",
  },
  {
    id: "strip--auto-uten-tider",
    page: "Ukestripa",
    state:
      "Auto på i en tom uke — flisen lyser ikke og sier «Ingen tider» (ingen tid er funnet på)",
    recipe: "`?goto=plan`, `autoRecordEnabled:true`, `slots: []`",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: { ...CHOSEN, autoRecordEnabled: true, slots: [] },
      goto: "plan",
    },
    wait: "strip-auto-needs-times",
  },
  {
    id: "strip--vekking",
    page: "Ukestripa",
    state:
      "Vekk-flisens popover — hva maskinen kan, «Legg inn vekking», prøv og logg",
    recipe: "`wake_capabilities`; klikk hjørneknappen `strip-wake-more`",
    boot: {
      fixtures: HEALTHY,
      settings: { ...CHOSEN, autoRecordEnabled: true, slots: SUNDAY_SLOT },
      goto: "plan",
    },
    wait: "strip-wake-more",
    act: async (page) => {
      await page.getByTestId("strip-wake-more").click();
      await expect(page.getByTestId("strip-wake-pop")).toBeVisible();
      await expect(page.getByTestId("strip-wake-caps")).toBeVisible();
    },
  },
  {
    id: "strip--varsel",
    page: "Ukestripa",
    state: "Varsel-flisens popover — status fra systemet og «Send testvarsel»",
    recipe:
      "`notification_permission:granted`; klikk `strip-notify-more`, så «Send testvarsel»",
    boot: {
      fixtures: {
        ...HEALTHY,
        notification_permission: "granted",
        notification_request_permission: "granted",
        notification_send_test: true,
      },
      settings: {
        ...CHOSEN,
        autoRecordEnabled: true,
        notifyStart: true,
        notifyStop: true,
        reminderMinutes: 10,
        slots: SUNDAY_SLOT,
      },
      goto: "plan",
    },
    wait: "strip-notify-more",
    act: async (page) => {
      await page.getByTestId("strip-notify-more").click();
      await page.getByTestId("strip-notify-test").click();
      await expect(page.getByTestId("strip-notify-test-word")).toBeVisible();
    },
  },
  {
    id: "strip--varsel-av-i-systemet",
    page: "Ukestripa",
    state:
      "Varsler slått på, men av i systemet — stripa sier fra og tilbyr systeminnstillingene",
    recipe: "`notification_permission:denied` + klikk på Varsel-flisen",
    boot: {
      fixtures: {
        ...HEALTHY,
        notification_permission: "denied",
        notification_request_permission: "denied",
        notification_open_settings: true,
      },
      settings: {
        ...CHOSEN,
        autoRecordEnabled: true,
        notifyStart: false,
        notifyStop: false,
        slots: SUNDAY_SLOT,
      },
      goto: "plan",
    },
    wait: "strip-tile-notify",
    act: async (page) => {
      await page.getByTestId("strip-tile-notify").click();
      await expect(page.getByTestId("strip-notify-denied")).toBeVisible();
    },
  },

  // ── Bannerne over Ta opp ──────────────────────────────────────────────────
  {
    id: "opptak--banner-avbrutt",
    page: "Ta opp",
    state: "Banner: opptaket ble avbrutt, med grunnen i klartekst",
    recipe: "`emit('recording-error', { code: 'device_disconnected' })`",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: CHOSEN,
      goto: "record",
    },
    wait: "record-start",
    act: async (page) => {
      await emitAt(page, "recording-error", {
        code: "device_disconnected",
        error: "device_disconnected",
        message: "cpal: device went away",
      });
      await expect(page.getByTestId("banner-recording-error")).toBeVisible();
      await settleVu(page);
    },
  },
  {
    id: "opptak--banner-lite-plass",
    page: "Ta opp",
    state: "Banner: under to timer igjen på disken",
    recipe: "`get_disk_space.freeBytes = 200 MB`",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        get_disk_space: { freeBytes: 200_000_000, totalBytes: 500e9 },
      },
      settings: CHOSEN,
      goto: "record",
    },
    wait: "banner-low-disk",
    act: async (page) => {
      await settleVu(page);
    },
  },
  {
    id: "opptak--banner-gikk-glipp",
    page: "Ta opp",
    state: "Banner: et planlagt opptak ble aldri tatt",
    recipe: "`emitEvent('scheduler://missed', [{ at, label }])`",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: CHOSEN,
      goto: "record",
    },
    wait: "record-start",
    act: async (page) => {
      await emitEventAt(page, "scheduler://missed", [
        { at: "2026-08-16T11:00:00", label: "Gudstjeneste" },
      ]);
      await expect(page.getByTestId("banner-missed")).toBeVisible();
      await settleVu(page);
    },
  },
  {
    id: "opptak--banner-forhandssjekk",
    page: "Ta opp",
    state: "Banner: forhåndssjekken fant noe å se på",
    recipe:
      "`media_permissions.microphone = denied` + ett `run_preflight`-funn",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        media_permissions: { microphone: "denied", camera: "authorized" },
        ffmpeg_health: { available: true, version: "7.1", path: "/x/ffmpeg" },
        run_preflight: [
          {
            severity: "warn",
            category: "disk",
            message: "Det er lite plass igjen på disken.",
          },
        ],
      },
      settings: CHOSEN,
      goto: "record",
    },
    wait: "banner-preflight",
    act: async (page) => {
      await settleVu(page);
    },
  },
  {
    id: "opptak--samtykkekort",
    page: "Ta opp",
    state: "Samtykkekortet — det ene spørsmålet om diagnosedata",
    recipe: "`telemetry_consent_get.needsPrompt = true`",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        telemetry_consent_get: {
          status: "never-asked",
          version: 2,
          decidedAt: null,
          currentVersion: 2,
          needsPrompt: true,
          active: false,
        },
      },
      settings: CHOSEN,
      goto: "record",
    },
    wait: "consent-card",
    act: async (page) => {
      await settleVu(page);
    },
  },
  {
    id: "opptak--kvittering",
    page: "Ta opp",
    state: "Kvitteringen etter et ferdig opptak",
    recipe: "`emit('recording-finished', { path })`",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: CHOSEN,
      goto: "record",
    },
    wait: "record-start",
    act: async (page) => {
      await emitAt(page, "recording-finished", {
        path: "/Users/frivillig/Opptak/2026-08-23 Gudstjeneste.mp3",
        file_path: "/Users/frivillig/Opptak/2026-08-23 Gudstjeneste.mp3",
        has_video: false,
        splitRestart: false,
      });
      await expect(page.getByTestId("record-done-file")).toBeVisible();
      await settleVu(page);
    },
  },

  // ══ OPPTAKSMODUS ══════════════════════════════════════════════════════════
  {
    id: "opptaksmodus--lyd",
    page: "Opptaksmodus",
    state:
      "Det tas opp uten kamera — enorm klokke, bred L/R-måler, «Hold» og «Til du stopper»",
    recipe:
      "`emit('recording-overlay-stop', {state:'recording'})`, klokka +42 min",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: CHOSEN,
      goto: "record",
    },
    wait: "record-start",
    act: async (page) => {
      await startOverlay(page);
      await expect(page.getByTestId("overlay-until-stop")).toBeVisible();
    },
  },
  {
    id: "opptaksmodus--kamera",
    page: "Opptaksmodus",
    state:
      "Det tas opp med kamera — bildet i ramme med rød kant, klokka som brikke, loddrett måler",
    recipe:
      "som over med `videoEnabled`, og en ekte JPEG ned kamerakanalen FØR opptaket (overlegget arver bildet)",
    boot: {
      fixtures: { ...CAMERA_FIXTURES, ...DEVICES, ...cameraFixtures() },
      settings: WITH_CAMERA,
      goto: "record",
    },
    wait: "record-camera-preview",
    act: async (page) => {
      await liveCamera(page, "record-camera-preview");
      await startOverlay(page);
      await expect(page.getByTestId("overlay-camera-preview")).toHaveAttribute(
        "data-phase",
        "live",
      );
    },
  },
  {
    id: "opptaksmodus--stopper-om",
    page: "Opptaksmodus",
    state: "Planlagt stopp om 15 minutter — «Stopper 11:52» med «+30» og «∞»",
    recipe: "`scheduled_stop_ms` = klokka + 57 min, så klokka flyttes til +42",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        recording_extend_autostop: true,
        recording_cancel_autostop: true,
      },
      settings: { ...CHOSEN, manualMaxMinutes: 60 },
      goto: "record",
    },
    wait: "record-start",
    act: async (page) => {
      await emitAt(page, "recording-overlay-stop", {
        state: "recording",
        scheduled_stop_ms: ATLAS_NOW.getTime() + 57 * 60_000,
      });
      await expect(page.getByTestId("recording-overlay")).toBeVisible();
      await advanceClock(page, 42);
      await expect(page.getByTestId("overlay-autostop-actions")).toBeVisible();
      await expect(page.getByTestId("overlay-autostop")).toContainText("11:");
      await settleVu(page, "recording-levels", -14, -6);
    },
  },
  {
    id: "opptaksmodus--stillhet",
    page: "Opptaksmodus",
    state: "Stillhetsvarsel som liten ikon + ord-brikke øverst",
    recipe: "overlegget oppe, så `emit('recording-silence', …)`",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: CHOSEN,
      goto: "record",
    },
    wait: "record-start",
    act: async (page) => {
      await startOverlay(page);
      await emitAt(page, "recording-silence", {
        code: "silence_detected",
        message: "Stillhet oppdaget i lydsignalet",
      });
      await expect(page.getByTestId("overlay-silence")).toBeVisible();
    },
  },
  {
    id: "opptaksmodus--skjermleser-stopp",
    page: "Opptaksmodus",
    state:
      "Stopp-bekreftelsen som VoiceOver/Narrator får i stedet for holdet — «Fortsett å ta opp» er primærvalget",
    recipe:
      "overlegget oppe, så et `click` uten pekertrykk (`detail: 0`) på `overlay-stop`",
    boot: {
      fixtures: { ...BASE, ...DEVICES, stop_recording: true },
      settings: CHOSEN,
      goto: "record",
    },
    wait: "record-start",
    act: async (page) => {
      await startOverlay(page);
      await page.getByTestId("overlay-stop").evaluate((el) => {
        el.dispatchEvent(
          new MouseEvent("click", {
            bubbles: true,
            cancelable: true,
            detail: 0,
          }),
        );
      });
      await expect(page.getByTestId("dialog")).toBeVisible();
    },
  },

  // ══ OPPTAK (lista) OG REDIGERING ══════════════════════════════════════════
  {
    id: "redigering--bibliotek",
    page: "Opptak og redigering",
    state: "Lista med opptak — mini-bølgeform, dato, lengde, notat",
    recipe: "tre rader i `recordings_list`, én med notat",
    boot: {
      fixtures: { ...BASE, ...DEVICES, recordings_list: LIBRARY_ROWS },
      settings: CHOSEN,
      goto: "recordings",
    },
    wait: "library-row",
    full: true,
    narrow: true,
  },
  {
    id: "redigering--bibliotek-tomt",
    page: "Opptak og redigering",
    state: "Lista er tom — og sier hva man gjør nå",
    recipe: "`recordings_list: []`",
    boot: {
      fixtures: { ...BASE, ...DEVICES, recordings_list: [] },
      settings: CHOSEN,
      goto: "recordings",
    },
    wait: "library-empty",
  },
  {
    id: "redigering--sok-ingen-treff",
    page: "Opptak og redigering",
    state: "Søket ga ingen treff",
    recipe: "fyll `library-search` med «finnesikke»",
    boot: {
      fixtures: { ...BASE, ...DEVICES, recordings_list: LIBRARY_ROWS },
      settings: CHOSEN,
      goto: "recordings",
    },
    wait: "library-row",
    act: async (page) => {
      await page.getByTestId("library-search").fill("finnesikke");
      await expect(page.getByTestId("library-no-hits")).toBeVisible();
    },
  },
  {
    id: "redigering--papirkurv",
    page: "Opptak og redigering",
    state:
      "Papirkurven (ikon oppe til høyre, med antall) — med «tømmes om N d»",
    recipe:
      "to rader i `trash_list`, så klikk søppelikonet `library-trash-open`",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        recordings_list: LIBRARY_ROWS,
        trash_list: [
          trashEntry(),
          trashEntry({
            id: "t2",
            name: "2026-07-19 Gudstjeneste.mp3",
            originalPath: "/Users/frivillig/Opptak/2026-07-19 Gudstjeneste.mp3",
            trashedPath: "/Users/frivillig/Opptak/.sundayrec-trash/t2.mp3",
            deletedAt: OLD_SUNDAY_MS - 7 * 86_400_000,
          }),
        ],
      },
      settings: CHOSEN,
      goto: "recordings",
    },
    wait: "library-trash-open",
    act: async (page) => {
      await page.getByTestId("library-trash-open").click();
      await expect(page.getByTestId("trash-row").first()).toBeVisible();
    },
    full: true,
  },
  {
    id: "redigering--papirkurv-tom",
    page: "Opptak og redigering",
    state: "Papirkurven er tom",
    recipe: "`trash_list: []`, så klikk søppelikonet `library-trash-open`",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        recordings_list: LIBRARY_ROWS,
        trash_list: [],
      },
      settings: CHOSEN,
      goto: "recordings",
    },
    wait: "library-trash-open",
    act: async (page) => {
      await page.getByTestId("library-trash-open").click();
      await expect(page.getByTestId("trash-empty")).toBeVisible();
    },
  },
  {
    id: "redigering--klipp",
    page: "Opptak og redigering",
    state:
      "Redigering, steget «Klipp» (lydopptak) — bølgeform med to håndtak, ingen video",
    recipe: "`openEditorWithRecording` på det fikstursydde opptaket",
    boot: { fixtures: editorScene(), settings: CHOSEN, goto: "recordings" },
    act: async (page) => {
      await openEditor(page);
      await expect(page.getByTestId("editor-handle-start")).toBeVisible();
    },
    full: true,
    narrow: true,
  },
  {
    id: "redigering--markering",
    page: "Opptak og redigering",
    state: "Dra i bølgeformen — markering med boblen «Klipp» og lengden",
    recipe: "åpne, så dra over bølgeformen fra 30 % til 40 %",
    boot: { fixtures: editorScene(), settings: CHOSEN, goto: "recordings" },
    act: async (page) => {
      await openEditor(page);
      const box = (await page.getByTestId("editor-canvas").boundingBox())!;
      const y = box.y + box.height / 2;
      await page.mouse.move(box.x + box.width * 0.3, y);
      await page.mouse.down();
      await page.mouse.move(box.x + box.width * 0.35, y, { steps: 4 });
      await page.mouse.move(box.x + box.width * 0.4, y, { steps: 4 });
      await page.mouse.up();
      await expect(page.getByTestId("editor-selection")).toBeVisible();
    },
    full: true,
  },
  {
    id: "redigering--kuttliste",
    page: "Opptak og redigering",
    state:
      "Et klipp er gjort — skravert område med angre-knapp, og kuttlista under",
    recipe: "åpne, dra en markering, trykk «Klipp»",
    boot: { fixtures: editorScene(), settings: CHOSEN, goto: "recordings" },
    act: async (page) => {
      await openEditor(page);
      const box = (await page.getByTestId("editor-canvas").boundingBox())!;
      const y = box.y + box.height / 2;
      await page.mouse.move(box.x + box.width * 0.3, y);
      await page.mouse.down();
      await page.mouse.move(box.x + box.width * 0.35, y, { steps: 4 });
      await page.mouse.move(box.x + box.width * 0.4, y, { steps: 4 });
      await page.mouse.up();
      await page.getByTestId("editor-selection-cut").click();
      await expect(page.getByTestId("editor-cut-list")).toBeVisible();
      await page.mouse.move(2, 2);
    },
    full: true,
  },
  {
    id: "redigering--stillhetsforslag",
    page: "Opptak og redigering",
    state:
      "Stillhetsforslag — stiplede områder med «Godta» og «Avvis»; ingenting er klippet",
    recipe: "`editor_peaks` med to stillheter, så klikk `editor-silence`",
    boot: {
      fixtures: editorScene({ editor_peaks: SILENT_PEAKS }),
      settings: CHOSEN,
      goto: "recordings",
    },
    act: async (page) => {
      await openEditor(page);
      await page.getByTestId("editor-silence").click();
      await expect(page.getByTestId("editor-silence-region")).toHaveCount(2);
      await page.mouse.move(2, 2);
    },
    full: true,
  },
  {
    id: "redigering--video",
    page: "Opptak og redigering",
    state:
      "Redigering av et videoopptak — stor 16:9-ramme med spill, tid og Inn/Ut, bølgeformen under",
    recipe:
      "`editor_load_recording.hasVideo = true`; `<video>` har ingen fil å vise i nettleseren, så rammen står svart",
    boot: {
      fixtures: editorScene(WITH_VIDEO_TRACK),
      settings: CHOSEN,
      goto: "recordings",
    },
    act: async (page) => {
      await openEditor(page);
      await expect(page.getByTestId("editor-video")).toBeVisible();
    },
    full: true,
  },
  {
    id: "redigering--lyd",
    page: "Opptak og redigering",
    state: "Steget «Lyd» — bryteren «Podkastklar lyd»",
    recipe: "åpne, så klikk `editor-steps-row-sound`",
    boot: { fixtures: editorScene(), settings: CHOSEN, goto: "recordings" },
    act: async (page) => {
      await openEditor(page);
      await expect(page.getByTestId("editor-handle-start")).toBeVisible();
      await page.getByTestId("editor-steps-row-sound").click();
      await expect(page.getByTestId("editor-sound")).toBeVisible();
    },
    full: true,
  },
  {
    id: "redigering--laster",
    page: "Opptak og redigering",
    state: "Arbeidsflaten laster — «Analyserer …»",
    recipe: "`editor_load_recording` som aldri svarer",
    boot: {
      fixtures: editorScene({
        editor_load_recording: fn("() => new Promise(() => {})"),
      }),
      settings: CHOSEN,
      goto: "recordings",
    },
    act: async (page) => {
      await openFile(page);
      await expect(page.getByTestId("editor-loading")).toBeVisible();
    },
  },
  {
    id: "redigering--feil",
    page: "Opptak og redigering",
    state: "Arbeidsflaten kunne ikke åpne fila",
    recipe:
      "BÅDE `editor_load_recording: null` OG `editor_peaks: null` — toppene er en ANDRE kilde til varighet",
    boot: {
      fixtures: editorScene({
        editor_load_recording: null,
        editor_peaks: null,
      }),
      settings: CHOSEN,
      goto: "recordings",
    },
    act: async (page) => {
      await openFile(page);
      await expect(page.getByTestId("editor-load-error")).toBeVisible();
    },
  },

  // ══ EKSPORTARKET ══════════════════════════════════════════════════════════
  {
    id: "eksport--valg",
    page: "Eksportarket",
    state:
      "Eksportarket (sideark fra høyre) — formatkortene MP3 «Podkast» og WAV «Arkiv», de tre flisene, mappe og «Lagre»",
    recipe: "åpne opptaket, så klikk `editor-export-open`",
    boot: {
      fixtures: editorScene({
        recordings_list: [recordingRow({ file_path: FILE })],
      }),
      settings: CHOSEN,
      goto: "recordings",
    },
    act: async (page) => {
      await openEditor(page);
      await page.getByTestId("editor-export-open").click();
      await expect(page.getByTestId("editor-export")).toBeVisible();
    },
  },
  {
    id: "eksport--video",
    page: "Eksportarket",
    state:
      "Eksportarket for et videoopptak — MP4 «Video» er med som tredje kort; med det valgt står intro/outro dempet",
    recipe: "`hasVideo = true`, så klikk MP4-kortet",
    boot: {
      fixtures: editorScene({
        ...WITH_VIDEO_TRACK,
        recordings_list: [recordingRow({ file_path: FILE })],
      }),
      settings: {
        ...CHOSEN,
        editorIntroPath: "/Users/frivillig/Musikk/Velkommen.wav",
      },
      goto: "recordings",
    },
    act: async (page) => {
      await openEditor(page);
      await page.getByTestId("editor-export-open").click();
      await page.getByTestId("editor-format-row-mp4").click();
      await expect(page.getByTestId("export-jingles-video-note")).toBeVisible();
    },
  },
  {
    id: "eksport--jingler",
    page: "Eksportarket",
    state: "Intro og outro valgt — filnavn på flisene",
    recipe:
      "innstillingene har `editorIntroPath` og `editorOutroPath`; åpne opptaket, så klikk `editor-export-open`",
    boot: {
      fixtures: editorScene({
        recordings_list: [recordingRow({ file_path: FILE })],
      }),
      settings: {
        ...CHOSEN,
        editorIntroPath: "/Users/frivillig/Musikk/Velkommen.wav",
        editorOutroPath: "/Users/frivillig/Musikk/Utgang.mp3",
      },
      goto: "recordings",
    },
    act: async (page) => {
      await openEditor(page);
      await page.getByTestId("editor-export-open").click();
      await expect(page.getByTestId("export-jingle-intro-name")).toHaveText(
        "Velkommen.wav",
      );
    },
  },
  {
    id: "eksport--jingle-meny",
    page: "Eksportarket",
    state: "Hjørneknappen på Intro-flisen — «Velg en annen fil» og «Fjern»",
    recipe: "som over, så klikk `export-jingle-intro-more`",
    boot: {
      fixtures: editorScene({
        recordings_list: [recordingRow({ file_path: FILE })],
      }),
      settings: {
        ...CHOSEN,
        editorIntroPath: "/Users/frivillig/Musikk/Velkommen.wav",
        editorOutroPath: "/Users/frivillig/Musikk/Utgang.mp3",
      },
      goto: "recordings",
    },
    act: async (page) => {
      await openEditor(page);
      await page.getByTestId("editor-export-open").click();
      await page.getByTestId("export-jingle-intro-more").click();
      await expect(page.getByTestId("export-jingle-intro-menu")).toBeVisible();
    },
  },
  {
    id: "eksport--kjorer",
    page: "Eksportarket",
    state: "Eksporten kjører — 40 %, med avbryt",
    recipe:
      "`editor_export` som henger (`EXPORT_HELD`) + `emit('editor-export-progress', {pct:40})`",
    boot: {
      fixtures: editorScene({
        recordings_list: [recordingRow({ file_path: FILE })],
        editor_export: EXPORT_HELD,
      }),
      settings: CHOSEN,
      goto: "recordings",
    },
    act: async (page) => {
      await openEditor(page);
      await page.getByTestId("editor-export-open").click();
      await page.getByTestId("editor-export-go").click();
      await expect(page.getByTestId("editor-exporting")).toBeVisible();
      await emitAt(page, "editor-export-progress", {
        pct: 40,
        phase: "encoding",
      });
      await expect(
        page.getByTestId("editor-export-progress-percent"),
      ).toHaveText("40%");
    },
  },
  {
    id: "eksport--kvittering",
    page: "Eksportarket",
    state: "Kvitteringen — fila som ble laget, og hvor den ligger",
    recipe: "`editor-export-go` med `editor_export` som svarer med én gang",
    boot: {
      fixtures: editorScene({
        recordings_list: [recordingRow({ file_path: FILE })],
      }),
      settings: CHOSEN,
      goto: "recordings",
    },
    act: async (page) => {
      await openEditor(page);
      await page.getByTestId("editor-export-open").click();
      await page.getByTestId("editor-export-go").click();
      await expect(page.getByTestId("editor-exported-file")).toBeVisible();
    },
  },

  // ══ TANNHJULET ════════════════════════════════════════════════════════════
  {
    id: "tannhjul--kort",
    page: "Tannhjulet",
    state:
      "Tannhjulet åpent over siden man kom fra — Språk, Oppdatering, Anonym statistikk og Om",
    recipe: "`?goto=settings` (kortet ligger over `underlay`)",
    boot: settingsScene(CHOSEN),
    wait: "gear-dialog",
    act: async (page) => {
      await expect(page.getByTestId("gear-row-language")).toBeVisible();
    },
    narrow: true,
  },
  {
    id: "tannhjul--oppdatering-klar",
    page: "Tannhjulet",
    state: "Oppdateringsraden: en versjon er lastet ned og venter",
    recipe: "`emit('update-downloaded', { version })`",
    boot: settingsScene({ ...CHOSEN, autoUpdate: true }),
    wait: "gear-row-update",
    act: async (page) => {
      await emitAt(page, "update-downloaded", { version: "0.18.0" });
      await expect(page.getByTestId("adv-update-install")).toBeVisible();
    },
  },
  {
    id: "tannhjul--oppdatering-feilet",
    page: "Tannhjulet",
    state: "Oppdateringsraden: sjekken gikk ikke",
    recipe: "`emit('update-error', 'boom')` — en BAR streng, ikke et objekt",
    boot: settingsScene({ ...CHOSEN, autoUpdate: true }),
    wait: "gear-row-update",
    act: async (page) => {
      await emitAt(page, "update-error", "boom");
      await expect(page.getByTestId("adv-update-error")).toBeVisible();
    },
  },
  {
    id: "tannhjul--statistikk",
    page: "Tannhjulet",
    state: "«Vis hva som sendes» på anonym statistikk — hele nyttelasten",
    recipe: "`telemetry_preview_payload` + klikk `adv-diag-preview`",
    boot: settingsScene(CHOSEN, {
      ...HEALTHY,
      telemetry_consent_get: {
        status: "granted",
        version: 2,
        decidedAt: 1_754_000_000_000,
        currentVersion: 2,
        needsPrompt: false,
        active: true,
      },
      telemetry_preview_payload: {
        json: '{\n  "app": "sundayrec",\n  "version": "0.32.0-beta.1",\n  "os": "macos"\n}',
        isNextPayload: true,
        isEmpty: false,
      },
    }),
    wait: "adv-diag",
    act: async (page) => {
      await page.getByTestId("adv-diag-preview").click();
      await expect(page.getByTestId("dialog-pre")).toBeVisible();
    },
  },
  {
    id: "tannhjul--lisenser",
    page: "Tannhjulet",
    state: "Om › «Vis lisenser» — de medfølgende lisensene i en ramme",
    recipe:
      "`licenses_notices` med en kort tekst, så klikk `adv-licenses-open`",
    boot: settingsScene(CHOSEN, {
      ...HEALTHY,
      licenses_notices:
        '<!doctype html><meta charset="utf-8"><body style="font-family:sans-serif;color:#222;padding:16px"><h2>FFmpeg (GPL)</h2><p>Kildekode: https://ffmpeg.org</p><h2>Tauri (MIT og Apache-2.0)</h2><p>Medfølger i appen.</p></body>',
    }),
    wait: "adv-licenses-open",
    act: async (page) => {
      await page.getByTestId("adv-licenses-open").click();
      await expect(page.getByTestId("licenses-viewer")).toBeVisible();
    },
  },

  // ══ FØRSTE GANG ═══════════════════════════════════════════════════════════
  {
    id: "forste-gang--1-lyd-lukket",
    page: "Første gang",
    state:
      "Hint 1 av 3 under kilde-brikka — lydporten er LUKKET: «Neste» venter på lyd",
    recipe: "`onboardingDone:false`, ingen VU-pakker",
    boot: { fixtures: FIRST_RUN_FIXTURES, settings: FIRST_RUN_SETTINGS },
    wait: "hint-bubble",
    narrow: true,
  },
  {
    id: "forste-gang--1-lyd-apen",
    page: "Første gang",
    state: "Hint 1 av 3 — lydporten er ÅPEN: vi hører noe",
    recipe: "`emit('vu-levels', { peak_dbfs: [-20,-20] })` — over −50 dBFS",
    boot: { fixtures: FIRST_RUN_FIXTURES, settings: FIRST_RUN_SETTINGS },
    wait: "hint-bubble",
    act: async (page) => {
      await settleVu(page, "vu-levels", -20, -20);
      await expect(page.getByTestId("hint-next")).not.toHaveAttribute(
        "aria-disabled",
        "true",
      );
    },
  },
  {
    id: "forste-gang--2-mappe",
    page: "Første gang",
    state:
      "Hint 2 av 3 — hvor skal opptakene? (lydarket åpnes med «Mer», mappe-raden pulserer)",
    recipe: "«Fortsett uten lyd», så ett steg fram",
    boot: { fixtures: FIRST_RUN_FIXTURES, settings: FIRST_RUN_SETTINGS },
    wait: "hint-bubble",
    act: async (page) => {
      await skipGate(page);
      await expect(page.getByTestId("setup-folder")).toBeVisible();
    },
  },
  {
    id: "forste-gang--3-plan",
    page: "Første gang",
    state: "Hint 3 av 3 — faste opptak, over kalenderknappen",
    recipe: "«Fortsett uten lyd», så to steg fram",
    boot: {
      fixtures: FIRST_RUN_FIXTURES,
      settings: {
        ...FIRST_RUN_SETTINGS,
        saveFolder: "/Users/frivillig/Opptak",
      },
    },
    wait: "hint-bubble",
    act: async (page) => {
      await skipGate(page);
      await nextSteps(page, 1);
      await expect(page.getByTestId("hint-bubble")).toHaveAttribute(
        "data-step",
        "plan",
      );
    },
    narrow: true,
  },

  // ══ SKALLET ═══════════════════════════════════════════════════════════════
  //
  // Fire av statuslinjens fem setninger. Den femte (`rec`) står bak
  // opptaksoverlegget og er ikke fotograferbar — se toppen av fila.
  {
    id: "skallet--status-ingen-kilde",
    page: "Skallet",
    state: "Statuslinja: «ingen lydkilde valgt» (gul)",
    recipe: "ingen `deviceId`",
    boot: {
      fixtures: { ...BASE, ...DEVICES, recordings_list: LIBRARY_ROWS },
      settings: SETTLED_SETTINGS,
      goto: "recordings",
    },
    wait: "status-line",
    act: async (page) => {
      await expect(page.getByTestId("status-line")).toHaveAttribute(
        "data-status",
        "nosound",
      );
    },
  },
  {
    id: "skallet--status-lite-plass",
    page: "Skallet",
    state: "Statuslinja: «lite plass igjen» (gul) — slår «ingen kilde»",
    recipe: "`freeBytes = 200 MB` og ingen valgt kilde samtidig",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        recordings_list: LIBRARY_ROWS,
        get_disk_space: { freeBytes: 200_000_000, totalBytes: 500e9 },
      },
      settings: SETTLED_SETTINGS,
      goto: "recordings",
    },
    wait: "status-line",
    act: async (page) => {
      await expect(page.getByTestId("status-line")).toHaveAttribute(
        "data-status",
        "lowdisk",
      );
    },
  },
  {
    id: "skallet--status-neste",
    page: "Skallet",
    state: "Statuslinja: «neste opptak …» (grå)",
    recipe: "kilde valgt + `autoRecordEnabled` + `scheduler_status.next`",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        recordings_list: LIBRARY_ROWS,
        scheduler_status: { next: NEXT_SERVICE_ISO },
      },
      settings: {
        ...CHOSEN,
        autoRecordEnabled: true,
        slots: [{ days: [0], start: "11:00", stop: "12:30", max: null }],
      },
      goto: "recordings",
    },
    wait: "status-line",
    act: async (page) => {
      await expect(page.getByTestId("status-line")).toHaveAttribute(
        "data-status",
        "next",
      );
    },
  },
  {
    id: "skallet--status-klar",
    page: "Skallet",
    state: "Statuslinja: «alt er klart» (grønn)",
    recipe: "kilde valgt, plass på disken, ingenting planlagt",
    boot: {
      fixtures: { ...BASE, ...DEVICES, recordings_list: LIBRARY_ROWS },
      settings: CHOSEN,
      goto: "recordings",
    },
    wait: "status-line",
    act: async (page) => {
      await expect(page.getByTestId("status-line")).toHaveAttribute(
        "data-status",
        "ready",
      );
    },
  },
  {
    id: "skallet--oppdateringsbanner",
    page: "Skallet",
    state: "Oppdateringsstripa — over den siden man ER på",
    recipe: "`emit('update-available', { version })` på Opptak",
    boot: {
      fixtures: { ...BASE, ...DEVICES, recordings_list: LIBRARY_ROWS },
      settings: { ...CHOSEN, autoUpdate: true },
      goto: "recordings",
    },
    wait: "library-row",
    act: async (page) => {
      await emitAt(page, "update-available", { version: "0.18.0" });
      await expect(page.getByTestId("banner-update")).toBeVisible();
    },
  },
  {
    id: "skallet--hydreringsfeil",
    page: "Skallet",
    state: "Innstillingene kunne ikke leses — aldri stille standardverdier",
    recipe: "`settings_get` kaster; feilstripa står under overskriften",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        settings_get: fn(`() => { throw new Error("store_unreadable") }`),
      },
      settings: CHOSEN,
      goto: "record",
    },
    wait: "hydrate-error",
  },
];
