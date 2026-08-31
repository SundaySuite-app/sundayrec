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
import { editorFixtures, EXPORT_HELD, FILE } from "../editor-fixtures";
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
  SPECIAL_DATE,
  stubCamera,
  stubClipboard,
} from "./harness";

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
 * `boot.goto` — a deep link in the OLD vocabulary; the router translates it.
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
 *    `overlegg--pagar` is what a recording actually looks like.
 *  - **The ASIO attribution card** and the `dshow` engine option: both gate on
 *    `currentOs() === "win"`, and `navigator.userAgentData.platform` beats any
 *    user-agent override, so Desktop Chrome can never be Windows here.
 *  - **Native pickers** («Åpne fil…», «Velg mappe», profil-import/-eksport).
 *    They go through `@tauri-apps/plugin-dialog`, which invokes directly and is
 *    not fixture-covered; the click is a silent no-op. The state BEFORE the
 *    click is photographed; the state after does not exist out here.
 *  - **Playback.** `asset://` is dead outside Tauri, so the editor's playback
 *    notice is always the honest «ikke tilgjengelig» one and the play button
 *    always `aria-disabled`.
 *  - **`adv-wake-caps` = «vet ikke ennå».** `wake_capabilities` goes through
 *    `call()`, which never rejects, so the component's null branch is only ever
 *    on screen for the frame before the promise resolves.
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
const BASE: Fixtures = {
  ...BOOT_FIXTURES,
  // The version in the bottom bar is in every single photograph, so it must not
  // be the browser tier's `0.10.0-e2e` placeholder — an atlas of v0.17.0 that
  // says 0.10.0 in the corner is a document that argues with itself. Read from
  // package.json rather than written down, so it can never go stale.
  app_info: { version: PKG_VERSION },
  update_check: { phase: "upToDate" },
  list_video_devices: [],
  list_devices: { video_inputs: [] },
};

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
  churchName: "Bryn menighet",
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

/**
 * A real, tiny 160×90 JPEG (ImageMagick, quality 70).
 *
 * The overlay reads the aspect ratio out of the SOF0 header and paints the
 * bytes into an `<img>`, so a header-only stub would measure correctly and then
 * render as a broken image. This one decodes.
 */
const FRAME_16_9 =
  "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAoHBwgHBgoICAgLCgoLDhgQDg0NDh0VFhEYIx8lJCIf" +
  "IiEmKzcvJik0KSEiMEExNDk7Pj4+JS5ESUM8SDc9Pjv/2wBDAQoLCw4NDhwQEBw7KCIoOzs7Ozs7" +
  "Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozv/wAARCABaAKADASIA" +
  "AhEBAxEB/8QAFwABAQEBAAAAAAAAAAAAAAAAAAIBB//EABUQAQEAAAAAAAAAAAAAAAAAAAAR/8QA" +
  "FwEBAQEBAAAAAAAAAAAAAAAAAAEEAv/EABQRAQAAAAAAAAAAAAAAAAAAAAD/2gAMAwEAAhEDEQA/" +
  "AOdUqaVuZ1UqaUFUqaUFUqaUFUqaUFUqaUFUqaUFUqaUFUqaUFUqaUGCRBQkBQkBQkBQkBQkBQkB" +
  "QkBQkBQkBlKmlcqqlTSgqlTSgqlTSgqlTSgqlTSgqlTSgqlTSgqlTSgqlTSgylTSoqqVNKCqVNKC" +
  "qVNKCqVNKCqVNKCqVNKCqVNKCqVNKCqVNKDKVNKiqpU0oKpU0oKpU0oKpU0oKpU0oKpU0oKpU0oK" +
  "pU0oKpU0oJpWCDaVgDaVgDaVgDaVgDaVgDaVgDaVgDaVgDaVgD//2Q==";

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

/** The editor's own fixtures, on top of the atlas base. */
function editorScene(over: Fixtures = {}): Fixtures {
  return { ...BASE, ...DEVICES, ...editorFixtures(over) };
}

/** Open a file through the same global the editor specs use. */
async function openFile(page: Page, path = FILE): Promise<void> {
  await page.evaluate(
    (f) =>
      (
        window as unknown as { openEditorWithFile: (p: string) => void }
      ).openEditorWithFile(f),
    path,
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

// ── Diagnose fixtures ───────────────────────────────────────────────────────

function report(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    markdown: "# SundayRec-diagnose\n\nAlt vel.\n",
    findings: [],
    savedTo:
      "/Users/frivillig/Library/Application Support/SundayRec/diagnose.md",
    captureOk: true,
    videoOk: null,
    captureProbeSkipped: null,
    ...over,
  };
}

function finding(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    code: "SR-AUDIO-01",
    severity: "critical",
    title: "Ingen lydenhet funnet",
    detail: "Verken Windows-lyd, ASIO eller ffmpeg fant en mikrofon/lydkort.",
    hint: "Sjekk at lydkortet er tilkoblet og driveren installert.",
    ...over,
  };
}

/** The four commands one diagnose run makes, answered by a healthy machine. */
const HEALTHY: Fixtures = {
  ...BASE,
  ...DEVICES,
  run_diagnostics: report(),
  diagnose_audio: {
    dshow: ["Behringer X32", "MacBook Pro-mikrofon"],
    wasapi: [],
    wasapiAvailable: false,
  },
  media_permissions: { camera: "authorized", microphone: "authorized" },
  ffmpeg_health: { available: true, version: "ffmpeg version 7.1", path: "/x" },
  wake_capabilities: {
    platform: "macos",
    canWakeFromSleep: true,
    canWakeFromOff: false,
    needsAdmin: true,
    knownIssues: [],
    recommendations: [],
  },
  email_status: { featureBuilt: true },
  email_has_smtp_password: false,
};

/** Innstillinger is the same surface every time; only the settings differ. */
function settingsScene(
  settings: Record<string, unknown>,
  fixtures: Fixtures = HEALTHY,
): BootOptions {
  return { fixtures, settings, goto: "settings:general" };
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
  await page.getByTestId("first-run-skip-sound").click();
}

/** Advance N times from wherever the sequence currently stands. */
async function nextSteps(page: Page, n: number): Promise<void> {
  for (let i = 0; i < n; i += 1) {
    await page.getByTestId("first-run-next").click();
  }
}

// ── The table ───────────────────────────────────────────────────────────────

export const SCENES: Scene[] = [
  // ══ OPPTAK ════════════════════════════════════════════════════════════════
  {
    id: "opptak--kald",
    page: "Opptak",
    state: "Kald start — ingen lydkilde valgt ennå",
    recipe: "ingen `deviceId`; Start er sperret og sier hvorfor",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: SETTLED_SETTINGS,
      goto: "home",
    },
    wait: "record-no-source",
    full: true,
    narrow: true,
  },
  {
    id: "opptak--klar",
    page: "Opptak",
    state: "Klar — kilde valgt, plass på disken, neste og siste opptak",
    recipe: "`deviceId:x32` + `scheduler_status.next` + tre opptak i lista",
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
        slots: [{ days: [0], start: "11:00", stop: "12:30", max: null }],
      },
      goto: "home",
    },
    wait: "record-source",
    act: async (page) => {
      await settleVu(page);
    },
    full: true,
    narrow: true,
  },
  {
    id: "opptak--kilde-borte",
    page: "Opptak",
    state: "Kilden er valgt, men ikke til stede nå",
    recipe: "`list_audio_devices` uten `x32`; Start er ÅPEN, med advarsel",
    boot: {
      fixtures: { ...BASE, list_audio_devices: [BUILT_IN] },
      settings: CHOSEN,
      goto: "home",
    },
    wait: "record-source-missing",
    full: true,
  },

  // ── De seks kortene, foldet ut på stedet ──────────────────────────────────
  {
    id: "opptak--kort-lyd",
    page: "Opptak",
    state: "Kilde-kortet foldet ut — hele «Hvilken lyd?» på stedet",
    recipe: "`?goto=settings:audio` → anker `sound`",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: CHOSEN,
      goto: "settings:audio",
    },
    wait: "control-sound-body",
    act: async (page) => {
      await settleVu(page);
    },
    full: true,
  },
  {
    id: "opptak--kort-mappe",
    page: "Opptak",
    state: "«Hvor skal opptakene?» foldet ut",
    recipe: "`?goto=settings:files` → anker `folder`",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: CHOSEN,
      goto: "settings:files",
    },
    wait: "control-folder-body",
    full: true,
  },
  {
    id: "opptak--kort-kvalitet",
    page: "Opptak",
    state: "«Hvilken kvalitet?» foldet ut",
    recipe: "klikk `control-quality-expand` (ingen gammel fane peker hit)",
    boot: {
      fixtures: { ...BASE, ...DEVICES },
      settings: CHOSEN,
      goto: "home",
    },
    wait: "control-quality",
    act: async (page) => {
      await page.getByTestId("control-quality-expand").click();
      await expect(page.getByTestId("control-quality-body")).toBeVisible();
    },
    full: true,
  },
  {
    id: "opptak--kort-kamera",
    page: "Opptak",
    state: "«Ta med kamera» foldet ut, med kameravalget",
    recipe:
      "`?goto=settings:video` + `videoEnabled:true` — kortet kan BARE foldes ut når tillegget er på",
    boot: {
      fixtures: CAMERA_FIXTURES,
      settings: WITH_CAMERA,
      goto: "settings:video",
    },
    pre: (page) => stubCamera(page),
    // ⚠️ `setup-camera-*`, ikke `control-camera-*`: CameraCard gir ControlCard
    // sin egen testId. Samme for auto-kortet.
    wait: "setup-camera-body",
    act: async (page) => {
      await expect(page.getByTestId("record-camera-preview")).toHaveAttribute(
        "data-phase",
        "live",
      );
      await settleVu(page);
    },
    full: true,
  },
  {
    id: "opptak--kort-auto",
    page: "Opptak",
    state: "«Ta opp automatisk» foldet ut, med to faste tider",
    recipe: "`?goto=schedule` → anker `auto`; to slots i innstillingene",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        scheduler_status: { next: NEXT_SERVICE_ISO },
      },
      settings: {
        ...CHOSEN,
        autoRecordEnabled: true,
        slots: [
          { days: [0], start: "11:00", stop: "12:30", max: null },
          { days: [2], start: "19:00", stop: "20:30", max: null },
        ],
      },
      goto: "schedule",
    },
    wait: "setup-auto-body",
    full: true,
  },
  {
    id: "opptak--kort-varsling",
    page: "Opptak",
    state: "«Varsling» foldet ut",
    recipe: "`?goto=settings:sharing` → anker `notify`",
    boot: {
      fixtures: { ...BASE, ...DEVICES, email_status: { featureBuilt: true } },
      settings: CHOSEN,
      goto: "settings:sharing",
    },
    wait: "control-notify-body",
    full: true,
  },

  // ── Kamerabildet, fase for fase ───────────────────────────────────────────
  {
    id: "opptak--kamera-live",
    page: "Opptak",
    state: "Kamerabildet står — fasen `live`",
    recipe: "`videoEnabled:true` + stubbet `getUserMedia` (canvas-strøm)",
    boot: { fixtures: CAMERA_FIXTURES, settings: WITH_CAMERA, goto: "home" },
    pre: (page) => stubCamera(page),
    wait: "record-camera-preview",
    act: async (page) => {
      // Vent til rammen SIER at bildet er der. Aldri en assertion på piksler:
      // `data-phase` er appens egen påstand, og det er den som kan være feil.
      await expect(page.getByTestId("record-camera-preview")).toHaveAttribute(
        "data-phase",
        "live",
      );
      // …og til målingen har landet, ellers fotograferes brikka mens den
      // fortsatt sier 0 × 0.
      await expect(
        page.getByTestId("record-camera-preview-badge"),
      ).toContainText("320");
      await settleVu(page);
    },
    full: true,
  },
  {
    id: "opptak--kamera-nektet",
    page: "Opptak",
    state: "Kamerabildet — fasen `denied` (OS-et sa nei)",
    recipe: "stubbet `getUserMedia` som kaster `NotAllowedError`",
    boot: { fixtures: CAMERA_FIXTURES, settings: WITH_CAMERA, goto: "home" },
    pre: (page) => stubCamera(page, "NotAllowedError"),
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
    page: "Opptak",
    state: "Kamerabildet — det lagrede kameraet finnes ikke",
    recipe: "`videoDeviceName` som ikke er blant `enumerateDevices`",
    boot: {
      fixtures: CAMERA_FIXTURES,
      settings: { ...WITH_CAMERA, videoDeviceName: "Sony HandyCam" },
      goto: "home",
    },
    pre: (page) => stubCamera(page),
    wait: "record-camera-preview",
    act: async (page) => {
      await expect(
        page.getByTestId("record-camera-preview-message"),
      ).toBeVisible();
      await settleVu(page);
    },
  },

  // ── Bannerne ──────────────────────────────────────────────────────────────
  {
    id: "opptak--banner-avbrutt",
    page: "Opptak",
    state: "Banner: opptaket ble avbrutt, med grunnen i klartekst",
    recipe: "`emit('recording-error', { code: 'device_disconnected' })`",
    boot: { fixtures: { ...BASE, ...DEVICES }, settings: CHOSEN, goto: "home" },
    wait: "record-source",
    act: async (page) => {
      await emitAt(page, "recording-error", {
        code: "device_disconnected",
        error: "device_disconnected",
        message: "cpal: device went away",
      });
      await expect(page.getByTestId("banner-recording-error")).toBeVisible();
      await settleVu(page);
    },
    full: true,
  },
  {
    id: "opptak--banner-lite-plass",
    page: "Opptak",
    state: "Banner: under to timer igjen på disken",
    recipe: "`get_disk_space.freeBytes = 200 MB`",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        get_disk_space: { freeBytes: 200_000_000, totalBytes: 500e9 },
      },
      settings: CHOSEN,
      goto: "home",
    },
    wait: "banner-low-disk",
    act: async (page) => {
      await settleVu(page);
    },
    full: true,
  },
  {
    id: "opptak--banner-gikk-glipp",
    page: "Opptak",
    state: "Banner: et planlagt opptak ble aldri tatt",
    recipe: "`emitEvent('scheduler://missed', [{ at, label }])`",
    boot: { fixtures: { ...BASE, ...DEVICES }, settings: CHOSEN, goto: "home" },
    wait: "record-source",
    act: async (page) => {
      await emitEventAt(page, "scheduler://missed", [
        { at: "2026-08-16T11:00:00", label: "Gudstjeneste" },
      ]);
      await expect(page.getByTestId("banner-missed")).toBeVisible();
      await settleVu(page);
    },
    full: true,
  },
  {
    id: "opptak--banner-forhandssjekk",
    page: "Opptak",
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
      goto: "home",
    },
    wait: "banner-preflight",
    act: async (page) => {
      await settleVu(page);
    },
    full: true,
  },
  {
    id: "opptak--samtykkekort",
    page: "Opptak",
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
      goto: "home",
    },
    wait: "consent-card",
    act: async (page) => {
      await settleVu(page);
    },
    full: true,
  },
  {
    id: "opptak--kvittering",
    page: "Opptak",
    state: "Kvitteringen etter et ferdig opptak",
    recipe: "`emit('recording-finished', { path })`",
    boot: { fixtures: { ...BASE, ...DEVICES }, settings: CHOSEN, goto: "home" },
    wait: "record-source",
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
    full: true,
  },

  // ══ OPPTAKSOVERLEGGET ═════════════════════════════════════════════════════
  {
    id: "overlegg--pagar",
    page: "Opptaksoverlegget",
    state: "Det tas opp — klokke, måler, kamerabilde og fakta",
    recipe:
      "`emit('recording-overlay-stop', {state:'recording'})`, klokka +42 min, ekte JPEG-frame",
    boot: {
      fixtures: {
        ...CAMERA_FIXTURES,
        ...DEVICES,
        recording_preview_frame: FRAME_16_9,
      },
      settings: WITH_CAMERA,
      goto: "home",
    },
    pre: (page) => stubCamera(page),
    wait: "record-source",
    act: async (page) => {
      await emitAt(page, "recording-overlay-stop", {
        state: "recording",
        scheduled_stop_ms: null,
      });
      await expect(page.getByTestId("recording-overlay")).toBeVisible();
      // Klokka står stille, så en teller som aldri hadde flyttet seg ville
      // stått på 00:00:00 i hver eneste kjøring. Å flytte den FASTE tiden gir
      // et tall som både er ekte og likt neste gang.
      await advanceClock(page, 42);
      await expect(page.getByTestId("overlay-timer")).toContainText("42:0");
      await settleVu(page, "recording-levels", -14, -6);
      // ⚠️ Tallene under måleren oppdateres av et intervall på ETT SEKUND, og
      // de finnes ikke i treet før den første tikken. Uten denne ventingen er
      // scenen et lotteri mellom «ingen tall», «−60 / −60» (tikken kom før
      // pakkene) og de riktige tallene — tre forskjellige bilder av den samme
      // tilstanden. Assertionen poller til intervallet har tatt igjen.
      await expect(page.getByTestId("overlay-vu-numbers")).toContainText(
        "-6 /",
      );
      await expect(page.getByTestId("overlay-camera-preview")).toHaveAttribute(
        "data-phase",
        "live",
      );
    },
  },
  {
    id: "overlegg--stopp-dialog",
    page: "Opptaksoverlegget",
    state: "Stopp-bekreftelsen — «Fortsett å ta opp» er primærvalget",
    recipe: "overlegget oppe, så klikk `overlay-stop`",
    boot: {
      fixtures: { ...BASE, ...DEVICES, stop_recording: true },
      settings: CHOSEN,
      goto: "home",
    },
    wait: "record-source",
    act: async (page) => {
      await emitAt(page, "recording-overlay-stop", {
        state: "recording",
        scheduled_stop_ms: null,
      });
      await expect(page.getByTestId("recording-overlay")).toBeVisible();
      await advanceClock(page, 42);
      await expect(page.getByTestId("overlay-timer")).toContainText("42:0");
      await settleVu(page, "recording-levels", -14, -6);
      // Se `overlegg--pagar`: tallene kommer på et sekundintervall.
      await expect(page.getByTestId("overlay-vu-numbers")).toContainText(
        "-6 /",
      );
      await page.getByTestId("overlay-stop").click();
      await expect(page.getByTestId("dialog")).toBeVisible();
    },
  },
  {
    id: "overlegg--nedtelling",
    page: "Opptaksoverlegget",
    state: "Auto-stopp om 15 minutter, med «+ 15 min» og «Avbryt»",
    recipe: "`scheduled_stop_ms` = klokka + 57 min, så klokka flyttes til +42",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        recording_extend_autostop: true,
        recording_cancel_autostop: true,
      },
      settings: { ...CHOSEN, manualMaxMinutes: 60 },
      goto: "home",
    },
    wait: "record-source",
    act: async (page) => {
      await emitAt(page, "recording-overlay-stop", {
        state: "recording",
        scheduled_stop_ms: ATLAS_NOW.getTime() + 57 * 60_000,
      });
      await expect(page.getByTestId("recording-overlay")).toBeVisible();
      await advanceClock(page, 42);
      await expect(page.getByTestId("overlay-autostop-actions")).toBeVisible();
      await expect(page.getByTestId("overlay-autostop")).toContainText("15:0");
      await settleVu(page, "recording-levels", -14, -6);
      // Se `overlegg--pagar`: tallene kommer på et sekundintervall.
      await expect(page.getByTestId("overlay-vu-numbers")).toContainText(
        "-6 /",
      );
    },
  },

  // ══ REDIGERING ════════════════════════════════════════════════════════════
  {
    id: "redigering--bibliotek",
    page: "Redigering",
    state: "Biblioteket med opptak — dato, lengde, notat",
    recipe: "tre rader i `recordings_list`, én med notat",
    boot: {
      fixtures: { ...BASE, ...DEVICES, recordings_list: LIBRARY_ROWS },
      settings: CHOSEN,
      goto: "search",
    },
    wait: "library-row",
    full: true,
    narrow: true,
  },
  {
    id: "redigering--bibliotek-tomt",
    page: "Redigering",
    state: "Biblioteket er tomt — og sier hva man gjør nå",
    recipe: "`recordings_list: []`",
    boot: {
      fixtures: { ...BASE, ...DEVICES, recordings_list: [] },
      settings: CHOSEN,
      goto: "search",
    },
    wait: "library-empty",
    full: true,
  },
  {
    id: "redigering--sok-ingen-treff",
    page: "Redigering",
    state: "Søket ga ingen treff",
    recipe: "fyll `library-search` med «finnesikke»",
    boot: {
      fixtures: { ...BASE, ...DEVICES, recordings_list: LIBRARY_ROWS },
      settings: CHOSEN,
      goto: "search",
    },
    wait: "library-row",
    act: async (page) => {
      await page.getByTestId("library-search").fill("finnesikke");
      await expect(page.getByTestId("library-no-hits")).toBeVisible();
    },
  },
  {
    id: "redigering--papirkurv",
    page: "Redigering",
    state: "Papirkurven — med «slettes om N dager»",
    recipe:
      "to rader i `trash_list`, så klikk `library-trash-open` (ikke nåbar via `?goto=`)",
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
      goto: "search",
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
    page: "Redigering",
    state: "Papirkurven er tom",
    recipe: "`trash_list: []`, så klikk `library-trash-open`",
    boot: {
      fixtures: {
        ...BASE,
        ...DEVICES,
        recordings_list: LIBRARY_ROWS,
        trash_list: [],
      },
      settings: CHOSEN,
      goto: "search",
    },
    wait: "library-trash-open",
    act: async (page) => {
      await page.getByTestId("library-trash-open").click();
      await expect(page.getByTestId("trash-empty")).toBeVisible();
    },
  },
  {
    id: "redigering--klipp",
    page: "Redigering",
    state: "Arbeidsflaten, steget «Klipp» — bølgeform og prekenforslag",
    recipe: "`openEditorWithFile` på det fikstursydde opptaket",
    boot: { fixtures: editorScene(), settings: CHOSEN, goto: "editor" },
    act: async (page) => {
      await openEditor(page);
      await expect(page.getByTestId("editor-suggestion")).toBeVisible();
    },
    full: true,
    narrow: true,
  },
  {
    id: "redigering--klipp-leter",
    page: "Redigering",
    state: "«Leter etter prekenen …» — analysen er ikke ferdig",
    recipe: "`editor_segments` som aldri svarer",
    boot: {
      fixtures: editorScene({
        editor_segments: fn("() => new Promise(() => {})"),
      }),
      settings: CHOSEN,
      goto: "editor",
    },
    act: async (page) => {
      await openEditor(page);
      await expect(page.getByTestId("editor-searching")).toBeVisible();
    },
    full: true,
  },
  {
    id: "redigering--kuttliste",
    page: "Redigering",
    state: "Kuttlista — det som blir borte, som rader man kan angre",
    recipe: "åpne, så klikk `editor-keep-sermon`",
    boot: { fixtures: editorScene(), settings: CHOSEN, goto: "editor" },
    act: async (page) => {
      await openEditor(page);
      await expect(page.getByTestId("editor-suggestion")).toBeVisible();
      await page.getByTestId("editor-keep-sermon").click();
      await expect(page.getByTestId("editor-cut-list")).toBeVisible();
    },
    full: true,
  },
  {
    id: "redigering--lyd",
    page: "Redigering",
    state: "Steget «Lyd» — profilene og hva de gjør",
    recipe: "åpne, så klikk `editor-steps-row-sound`",
    boot: { fixtures: editorScene(), settings: CHOSEN, goto: "editor" },
    act: async (page) => {
      await openEditor(page);
      // ⚠️ VENT PÅ ANALYSEN FØRST. «Hør et utsnitt» starter der prekenen er
      // (`refreshListenStart` → `sermonWindow`), og prekenvinduet fylles av et
      // ASYNKRONT `editor_segments`-svar. Bytter man steg før det har landet,
      // står tidspunktet på 0:04:50 (midten av opptaket); etterpå står det på
      // 0:05:05 (midten av prekenen). Uten denne ventingen fotograferte
      // scenen tilfeldig det ene eller det andre.
      await expect(page.getByTestId("editor-suggestion")).toBeVisible();
      await page.getByTestId("editor-steps-row-sound").click();
      await expect(page.getByTestId("editor-sound")).toBeVisible();
    },
    full: true,
  },
  {
    id: "redigering--mikser",
    page: "Redigering",
    state: "Mikseren åpen — de sju trinnene bak profilen",
    recipe: "steget «Lyd», så `editor-mixer-open` + `editor-mixer-toggle`",
    boot: { fixtures: editorScene(), settings: CHOSEN, goto: "editor" },
    act: async (page) => {
      await openEditor(page);
      // Se `redigering--lyd`: prekenvinduet må ha landet før steget byttes.
      await expect(page.getByTestId("editor-suggestion")).toBeVisible();
      await page.getByTestId("editor-steps-row-sound").click();
      await page.getByTestId("editor-mixer-open").click();
      await expect(page.getByTestId("editor-mixer-stages")).toBeVisible();
      await page.getByTestId("editor-mixer-toggle").click();
      await expect(page.getByTestId("editor-mixer-toggle")).toHaveAttribute(
        "aria-checked",
        "true",
      );
    },
    full: true,
  },
  {
    id: "redigering--laster",
    page: "Redigering",
    state: "Arbeidsflaten laster — «Analyserer …»",
    recipe: "`editor_load_recording` som aldri svarer",
    boot: {
      fixtures: editorScene({
        editor_load_recording: fn("() => new Promise(() => {})"),
      }),
      settings: CHOSEN,
      goto: "editor",
    },
    act: async (page) => {
      await openFile(page);
      await expect(page.getByTestId("editor-loading")).toBeVisible();
    },
  },
  {
    id: "redigering--feil",
    page: "Redigering",
    state: "Arbeidsflaten kunne ikke åpne fila",
    recipe:
      "BÅDE `editor_load_recording: null` OG `editor_peaks: null` — toppene er en ANDRE kilde til varighet",
    boot: {
      fixtures: editorScene({
        editor_load_recording: null,
        editor_peaks: null,
      }),
      settings: CHOSEN,
      goto: "editor",
    },
    act: async (page) => {
      await openFile(page);
      await expect(page.getByTestId("editor-load-error")).toBeVisible();
    },
  },

  // ══ EKSPORTERING ══════════════════════════════════════════════════════════
  {
    id: "eksport--tom",
    page: "Eksportering",
    state: "Ingen fil åpen — siste opptak med én knapp, og en velger",
    recipe: "`?goto=export` med tre opptak i lista",
    boot: {
      fixtures: editorScene({ recordings_list: LIBRARY_ROWS }),
      settings: CHOSEN,
      goto: "export",
    },
    wait: "export-last",
    full: true,
    narrow: true,
  },
  {
    id: "eksport--ingenting",
    page: "Eksportering",
    state: "Ingen opptak i det hele tatt",
    recipe: "`recordings_list: []`",
    boot: {
      fixtures: editorScene({ recordings_list: [] }),
      settings: CHOSEN,
      goto: "export",
    },
    wait: "export-empty",
  },
  {
    id: "eksport--valg",
    page: "Eksportering",
    state: "Valgene — format, hvor, og hva som blir laget",
    recipe: "`export-pick-use` på første rad i velgeren",
    boot: {
      fixtures: editorScene({
        recordings_list: [recordingRow({ file_path: FILE })],
      }),
      settings: CHOSEN,
      goto: "export",
    },
    wait: "export-last",
    act: async (page) => {
      await page.getByTestId("export-last-open").click();
      await expect(page.getByTestId("editor-export")).toBeVisible();
    },
    full: true,
  },
  {
    id: "eksport--kjorer",
    page: "Eksportering",
    state: "Eksporten kjører — 40 %, med avbryt",
    recipe:
      "`editor_export` som henger (`EXPORT_HELD`) + `emit('editor-export-progress', {pct:40})`",
    boot: {
      fixtures: editorScene({
        recordings_list: [recordingRow({ file_path: FILE })],
        editor_export: EXPORT_HELD,
      }),
      settings: CHOSEN,
      goto: "export",
    },
    wait: "export-last",
    act: async (page) => {
      await page.getByTestId("export-last-open").click();
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
    page: "Eksportering",
    state: "Kvitteringen — fila som ble laget, og hvor den ligger",
    recipe: "`editor-export-go` med `editor_export` som svarer med én gang",
    boot: {
      fixtures: editorScene({
        recordings_list: [recordingRow({ file_path: FILE })],
      }),
      settings: CHOSEN,
      goto: "export",
    },
    wait: "export-last",
    act: async (page) => {
      await page.getByTestId("export-last-open").click();
      await page.getByTestId("editor-export-go").click();
      await expect(page.getByTestId("editor-exported-file")).toBeVisible();
    },
    full: true,
  },

  // ══ INNSTILLINGER ═════════════════════════════════════════════════════════
  {
    id: "innstillinger--landing",
    page: "Innstillinger",
    state: "Hele flaten — kirkeprofil øverst, Avansert under",
    recipe: "`?goto=settings:general` på en frisk maskin",
    boot: settingsScene(CHOSEN),
    wait: "setup-church",
    full: true,
    narrow: true,
  },
  {
    id: "innstillinger--opptakskortet",
    page: "Innstillinger",
    state: "Avansert › Opptak, med alle betingede rader åpne",
    recipe: "`stopOnSilence`, `splitMinutes` og `autoDeleteDays` alle satt",
    boot: settingsScene({
      ...CHOSEN,
      preRollSeconds: 30,
      stopOnSilence: true,
      splitMinutes: 90,
      autoDeleteDays: 60,
    }),
    wait: "advanced-recording",
    act: async (page) => {
      await expect(page.getByTestId("adv-split-every")).toBeVisible();
      await page.getByTestId("advanced-recording").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "innstillinger--systemkortet",
    page: "Innstillinger",
    state:
      "Avansert › System — diagnosedata, oppdatering, logg, profil, diagnose",
    recipe: "rull til `advanced-system`",
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
    }),
    wait: "advanced-system",
    act: async (page) => {
      await page.getByTestId("advanced-system").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "innstillinger--oppdatering-klar",
    page: "Innstillinger",
    state: "Oppdateringsraden: en versjon er lastet ned og venter",
    recipe: "`emit('update-downloaded', { version })`",
    boot: settingsScene({ ...CHOSEN, autoUpdate: true }),
    wait: "adv-update",
    act: async (page) => {
      await emitAt(page, "update-downloaded", { version: "0.18.0" });
      await expect(page.getByTestId("adv-update-install")).toBeVisible();
      await page.getByTestId("adv-update").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "innstillinger--oppdatering-feilet",
    page: "Innstillinger",
    state: "Oppdateringsraden: sjekken gikk ikke",
    recipe: "`emit('update-error', 'boom')` — en BAR streng, ikke et objekt",
    boot: settingsScene({ ...CHOSEN, autoUpdate: true }),
    wait: "adv-update",
    act: async (page) => {
      await emitAt(page, "update-error", "boom");
      await expect(page.getByTestId("adv-update-error")).toBeVisible();
      await page.getByTestId("adv-update").scrollIntoViewIfNeeded();
    },
  },
  {
    id: "innstillinger--telemetri-dialog",
    page: "Innstillinger",
    state: "«Vis» på diagnosedata — hva som faktisk sendes",
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
        json: '{\n  "app": "sundayrec",\n  "version": "0.17.0",\n  "os": "macos"\n}',
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
    id: "innstillinger--smtp-uten-passord",
    page: "Innstillinger",
    state: "Varsling på e-post — ingen SMTP satt opp ennå",
    recipe: "`email_has_smtp_password: false`, tomme SMTP-felter",
    boot: settingsScene({
      ...CHOSEN,
      emailSmtp: "",
      emailSmtpUser: "",
      emailSmtpFrom: "",
    }),
    wait: "advanced-smtp",
    act: async (page) => {
      await page.getByTestId("advanced-smtp").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "innstillinger--smtp-med-passord",
    page: "Innstillinger",
    state: "Varsling på e-post — passordet ligger i nøkkelringen",
    recipe: "`email_has_smtp_password: true` + utfylte SMTP-felter",
    boot: settingsScene(
      {
        ...CHOSEN,
        emailSmtp: "smtp.kirke.no",
        emailSmtpUser: "varsler@kirke.no",
        emailSmtpFrom: "opptak@kirke.no",
      },
      { ...HEALTHY, email_has_smtp_password: true },
    ),
    wait: "advanced-smtp",
    act: async (page) => {
      await page.getByTestId("advanced-smtp").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "innstillinger--tidsplan",
    page: "Innstillinger",
    state: "Tidsplanen — to faste tider, ett spesialopptak, vekking",
    recipe: "`slots` + `specialRecordings` + `wake_capabilities`",
    boot: settingsScene({
      ...CHOSEN,
      autoRecordEnabled: true,
      slots: [
        { days: [0], start: "11:00", stop: "12:30", max: null },
        { days: [2], start: "19:00", stop: "20:30", max: null },
      ],
      specialRecordings: [
        {
          id: null,
          date: SPECIAL_DATE,
          name: "Julaften",
          start: "16:00",
          stop: "17:30",
          deviceId: null,
        },
      ],
    }),
    wait: "advanced-schedule",
    act: async (page) => {
      await page.getByTestId("advanced-schedule").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "innstillinger--tidsplan-tom",
    page: "Innstillinger",
    state: "Tidsplanen — ingen faste tider ennå",
    recipe: "`slots: []`, `specialRecordings: []`",
    boot: settingsScene({
      ...CHOSEN,
      slots: [],
      specialRecordings: [],
    }),
    wait: "advanced-schedule",
    act: async (page) => {
      await page.getByTestId("advanced-schedule").scrollIntoViewIfNeeded();
    },
    full: true,
  },

  // ── Diagnoseraden (V1/PR2) ────────────────────────────────────────────────
  {
    id: "diagnose--hvile",
    page: "Innstillinger › Diagnose",
    state: "I hvile — «Kjør» og «Test-opptak», ingenting annet",
    recipe: "diagnosen åpner en enhet, så den kjører aldri av seg selv",
    boot: settingsScene({ ...CHOSEN, deviceName: "Behringer X32" }),
    wait: "adv-diagnose",
    act: async (page) => {
      await page.getByTestId("adv-diagnose").scrollIntoViewIfNeeded();
      await expect(page.getByTestId("adv-diagnose-result")).toHaveCount(0);
    },
  },
  {
    id: "diagnose--resultat",
    page: "Innstillinger › Diagnose",
    state: "Resultatet — fem statusrader, funn, enhetsliste, kopiknapp",
    recipe: "`run_diagnostics` med tre funn, så klikk `adv-diagnose-run`",
    boot: settingsScene(
      { ...CHOSEN, deviceName: "Behringer X32" },
      {
        ...HEALTHY,
        run_diagnostics: report({
          findings: [
            finding({
              code: "SR-DISK-01",
              severity: "warning",
              title: "Marsboere oppdaget",
              hint: "Ikke få panikk.",
              detail: "Bare 4,2 GB ledig på /Users/frivillig/Opptak.",
            }),
            finding({
              code: "SR-PERM-01",
              severity: "critical",
              detail: "macOS nekter mikrofontilgang for SundayRec.",
            }),
            finding({
              // UKJENT kode ⇒ motorens egen prosa står, som den skal.
              code: "SR-FRA-FRAMTIDEN-01",
              severity: "warning",
              title: "Noe helt nytt fra en nyere bakende",
              detail: "En detalj bare motoren kjenner.",
              hint: "Motorens eget råd.",
            }),
          ],
        }),
      },
    ),
    wait: "adv-diagnose",
    act: async (page) => {
      await page.getByTestId("adv-diagnose-run").click();
      await expect(page.getByTestId("adv-diagnose-result")).toBeVisible();
      // Enhetslista er en `<details>` og står lukket — den er halve poenget
      // med raden, så den åpnes før lukkeren går.
      await page.getByTestId("adv-diagnose-devices").locator("summary").click();
      await page.getByTestId("adv-diagnose").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "diagnose--proven-hoppet-over",
    page: "Innstillinger › Diagnose",
    state: "Lydprøven ble ikke kjørt — den tredje tilstanden, ærlig",
    recipe: "`captureOk: null` + `captureProbeSkipped` med motorens egen grunn",
    boot: settingsScene(
      { ...CHOSEN, deviceName: "Behringer X32" },
      {
        ...HEALTHY,
        run_diagnostics: report({
          captureOk: null,
          captureProbeSkipped: "en annen klient holder enheten",
        }),
        diagnose_audio: {
          dshow: ["MacBook Pro-mikrofon"],
          wasapi: [],
          wasapiAvailable: false,
        },
      },
    ),
    wait: "adv-diagnose",
    act: async (page) => {
      await page.getByTestId("adv-diagnose-run").click();
      await expect(
        page.getByTestId("adv-diagnose-probe-skipped"),
      ).toBeVisible();
      await page.getByTestId("adv-diagnose").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "diagnose--ipc-ring",
    page: "Innstillinger › Diagnose",
    state: "Kommandoer som ikke svarte denne økten",
    recipe: "la `get_disk_space` og `recordings_list` kaste, så kjør",
    boot: settingsScene(
      { ...CHOSEN, deviceName: "Behringer X32" },
      {
        ...HEALTHY,
        get_disk_space: fn(`() => { throw new Error("disken svarte ikke") }`),
        recordings_list: fn(`() => { throw new Error("basen er låst") }`),
      },
    ),
    wait: "adv-diagnose",
    act: async (page) => {
      await page.getByTestId("adv-diagnose-run").click();
      await expect(page.getByTestId("adv-diagnose-ipc")).toBeVisible();
      await page.getByTestId("adv-diagnose-ipc").locator("summary").click();
      await page.getByTestId("adv-diagnose").scrollIntoViewIfNeeded();
    },
    full: true,
  },
  {
    id: "diagnose--feilet",
    page: "Innstillinger › Diagnose",
    state: "Diagnosen kunne ikke kjøres",
    recipe: "`run_diagnostics` kaster — den går utenom `call()`s fallback",
    boot: settingsScene(
      { ...CHOSEN, deviceName: "Behringer X32" },
      {
        ...HEALTHY,
        run_diagnostics: fn(`() => { throw new Error("basen svarte ikke") }`),
      },
    ),
    wait: "adv-diagnose",
    act: async (page) => {
      await page.getByTestId("adv-diagnose-run").click();
      await expect(page.getByTestId("adv-diagnose-failed")).toBeVisible();
      await page.getByTestId("adv-diagnose").scrollIntoViewIfNeeded();
    },
  },
  {
    id: "diagnose--testopptak",
    page: "Innstillinger › Diagnose",
    state: "Test-opptaket ble gjennomført",
    recipe: "`run_test_recording` → `{ ok: true, signal: 'normal' }`",
    boot: settingsScene(
      { ...CHOSEN, deviceName: "Behringer X32" },
      {
        ...HEALTHY,
        run_test_recording: {
          ok: true,
          error: null,
          sizeBytes: 1_000_000,
          signal: "normal",
        },
      },
    ),
    wait: "adv-diagnose",
    act: async (page) => {
      await page.getByTestId("adv-diagnose-test").click();
      await expect(page.getByTestId("adv-diagnose-test-result")).toBeVisible();
      await page.getByTestId("adv-diagnose").scrollIntoViewIfNeeded();
    },
  },
  {
    id: "diagnose--kopiert",
    page: "Innstillinger › Diagnose",
    state: "«Kopier full rapport» — kvitteringen som forsvinner av seg selv",
    recipe: "stubbet utklippstavle + klikk `adv-diagnose-copy`",
    boot: settingsScene({ ...CHOSEN, deviceName: "Behringer X32" }),
    pre: (page) => stubClipboard(page),
    wait: "adv-diagnose",
    act: async (page) => {
      await page.getByTestId("adv-diagnose-run").click();
      await page.getByTestId("adv-diagnose-copy").click();
      // Aldri på ORDET: scenen fotograferes på begge språk, og en assertion på
      // norsk tekst gjør den engelske kjøringen rød uten at noe er galt.
      await expect(
        page.locator('[data-testid^="toast-"][data-testid$="-message"]'),
      ).toBeVisible();
      await page.getByTestId("adv-diagnose").scrollIntoViewIfNeeded();
    },
  },
  {
    id: "diagnose--fra-menylinjen",
    page: "Innstillinger › Diagnose",
    state: "Menylinjens «Kjør diagnose» — bytter skjerm OG kjører",
    recipe: "start på OPPTAK, `emitEvent('tray://action', 'run-diagnostics')`",
    boot: {
      fixtures: HEALTHY,
      settings: { ...CHOSEN, deviceName: "Behringer X32" },
      goto: "home",
    },
    wait: "record-source",
    act: async (page) => {
      await emitEventAt(page, "tray://action", "run-diagnostics");
      await expect(page.getByTestId("adv-diagnose-result")).toBeVisible();
    },
    full: true,
  },

  // ══ FØRSTE GANG ═══════════════════════════════════════════════════════════
  {
    id: "forste-gang--1-lyd-lukket",
    page: "Første gang",
    state: "Steg 1 av 5 — lydporten er LUKKET: «Neste» venter på lyd",
    recipe: "`onboardingDone:false`, ingen VU-pakker",
    boot: { fixtures: FIRST_RUN_FIXTURES, settings: FIRST_RUN_SETTINGS },
    wait: "first-run",
    full: true,
    narrow: true,
  },
  {
    id: "forste-gang--1-lyd-apen",
    page: "Første gang",
    state: "Steg 1 av 5 — lydporten er ÅPEN: vi hører noe",
    recipe: "`emit('vu-levels', { peak_dbfs: [-20,-20] })` — over −50 dBFS",
    boot: { fixtures: FIRST_RUN_FIXTURES, settings: FIRST_RUN_SETTINGS },
    wait: "first-run",
    act: async (page) => {
      await settleVu(page, "vu-levels", -20, -20);
      await expect(page.getByTestId("first-run-next")).not.toHaveAttribute(
        "aria-disabled",
        "true",
      );
    },
    full: true,
  },
  {
    id: "forste-gang--2-mappe",
    page: "Første gang",
    state: "Steg 2 av 5 — hvor skal opptakene?",
    recipe: "«Fortsett uten lyd», så ett steg fram",
    boot: { fixtures: FIRST_RUN_FIXTURES, settings: FIRST_RUN_SETTINGS },
    wait: "first-run",
    act: async (page) => {
      await skipGate(page);
      await expect(page.getByTestId("setup-folder")).toBeVisible();
    },
    full: true,
  },
  {
    id: "forste-gang--3-kvalitet",
    page: "Første gang",
    state: "Steg 3 av 5 — hvilken kvalitet?",
    recipe: "«Fortsett uten lyd», så to steg fram",
    boot: { fixtures: FIRST_RUN_FIXTURES, settings: FIRST_RUN_SETTINGS },
    wait: "first-run",
    act: async (page) => {
      await skipGate(page);
      await nextSteps(page, 1);
      await expect(page.getByTestId("setup-quality")).toBeVisible();
    },
    full: true,
  },
  {
    id: "forste-gang--4-kirke",
    page: "Første gang",
    state: "Steg 4 av 5 — hva heter menigheten?",
    recipe: "«Fortsett uten lyd», så tre steg fram",
    boot: { fixtures: FIRST_RUN_FIXTURES, settings: FIRST_RUN_SETTINGS },
    wait: "first-run",
    act: async (page) => {
      await skipGate(page);
      await nextSteps(page, 2);
      await expect(page.getByTestId("setup-church")).toBeVisible();
    },
    full: true,
  },
  {
    id: "forste-gang--5-varsling",
    page: "Første gang",
    state: "Steg 5 av 5 — hvem skal få beskjed?",
    recipe: "«Fortsett uten lyd», så fire steg fram",
    boot: {
      fixtures: { ...FIRST_RUN_FIXTURES, email_status: { featureBuilt: true } },
      settings: FIRST_RUN_SETTINGS,
    },
    wait: "first-run",
    act: async (page) => {
      await skipGate(page);
      await nextSteps(page, 3);
      await expect(page.getByTestId("setup-notify")).toBeVisible();
    },
    full: true,
  },
  {
    id: "forste-gang--6-sjekkliste",
    page: "Første gang",
    state: "«Klar til søndag» — de fem svarene, med det som mangler i gult",
    recipe: "«Fortsett uten lyd» + fire steg; `notify` er ikke satt opp",
    boot: {
      fixtures: FIRST_RUN_FIXTURES,
      settings: {
        ...FIRST_RUN_SETTINGS,
        saveFolder: "/Users/frivillig/Opptak",
        churchName: "Bryn menighet",
      },
    },
    wait: "first-run",
    act: async (page) => {
      await skipGate(page);
      await nextSteps(page, 4);
      await expect(page.getByTestId("first-run-open")).toBeVisible();
    },
    full: true,
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
      goto: "search",
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
      goto: "search",
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
      goto: "search",
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
      goto: "search",
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
    recipe: "`emit('update-available', { version })` på Redigering",
    boot: {
      fixtures: { ...BASE, ...DEVICES, recordings_list: LIBRARY_ROWS },
      settings: { ...CHOSEN, autoUpdate: true },
      goto: "search",
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
      goto: "home",
    },
    wait: "hydrate-error",
    full: true,
  },
];
