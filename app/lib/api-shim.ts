// window.api shim — maps the OLD Electron preload surface onto the Tauri backend.
//
// Loaded as a module script BEFORE ./main.ts in index.html, so `window.api`
// exists before the renderer boots.
//
// Most methods below are wired to real Tauri `invoke()` commands (the contract
// is documented in reference/hooks.ts + reference/bindings). Each wired method
// calls the backend through `call()` and falls back to a safe default on any
// error, so a missing/mismatched command degrades to the old empty-state
// instead of throwing. A handful of methods are still deliberate stubs — no
// Rust command backs them (yet, or ever, for a feature that didn't survive the
// port) — and return a fixed value; those are called out inline where they sit.
//
// NOTE: NOTHING in the renderer opens an audio input device any more. Audio
// metering and audio-device enumeration are both backend-only:
//   1. Every meter (Home, Direkte, onboarding, the channel grid) reads ONE
//      backend VU session via startVu/stopVu + the `vu-levels` event, shared
//      through audio/vu-feed.ts. getUserMedia capped a device at 2 channels and
//      — worse — left WebKit holding a multi-channel input in that 2-channel
//      format long after the meter was "stopped" (the 2026-07-31 Qu-5 incident).
//   2. The recording overlay's meter is driven by the ACTIVE RECORDING's own
//      `recording://levels` telemetry (see pages/recording.ts), so the mic keeps
//      exactly one owner for the whole take.
//   3. Audio input devices come from `list_audio_devices` (below); only the
//      CAMERA preview is still client-side getUserMedia (pages/home.ts), which
//      is a video device and never contends for the microphone.

import {
  invoke as tauriInvoke,
  convertFileSrc,
  isTauri,
} from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  open as openDialog,
  save as saveDialog,
} from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { t } from "./i18n";
import type { PruneSummary } from "../../legacy/bindings/PruneSummary";
import type { TrashEntry } from "../../legacy/bindings/TrashEntry";
import type { Settings } from "../../legacy/bindings/Settings";
import type { WakeResult } from "../../legacy/bindings/WakeResult";
import type { WakeStatus } from "../../legacy/bindings/WakeStatus";
import { SETTINGS_DEFAULTS } from "./settings-defaults";
import { migrateLegacySettingsOnce } from "./migrate-legacy-settings";
import {
  createIpcFailureState,
  recentFailures,
  recordFailure,
  type IpcFailure,
} from "./ipc-failures-core";
import {
  FIXTURE_GLOBAL,
  FIXTURE_QUERY_PARAM,
  fixturesHonored,
  lookupFixture,
  readFixture,
  type FixtureGate,
  type FixtureMap,
} from "./fixtures-core";
import { createNotifierSlot, type ShimNotifier } from "./shim-notifier-core";
import { parseGoto } from "./goto-core";

// Broad, VLC-like accept lists — the bundled ffmpeg demuxes all of these, and
// the loader falls back to a full-fidelity AAC proxy (streamed from disk, same
// as the original) for anything the webview can't decode directly. Keep these in
// sync with the drag-drop sets in editor-page.ts / editor/state.ts.
const AUDIO_EXT = [
  "mp3",
  "mp1",
  "mp2",
  "wav",
  "flac",
  "aac",
  "m4a",
  "m4b",
  "m4r",
  "ogg",
  "oga",
  "opus",
  "aiff",
  "aif",
  "wma",
  "mka",
  "ac3",
  "eac3",
  "amr",
  "3ga",
  "caf",
  "wv",
  "tta",
  "au",
  "snd",
  "ape",
  "dts",
  "mpc",
  "ra",
  "ram",
  "spx",
  "gsm",
];
const VIDEO_EXT = [
  "mp4",
  "mov",
  "mkv",
  "m4v",
  "webm",
  "avi",
  "wmv",
  "ts",
  "mts",
  "m2ts",
  "flv",
  "3gp",
  "asf",
  "f4v",
];
// Everything the editor can ingest — audio OR video. The loader probes/decodes
// per file, so the picker should be as accepting as possible.
const MEDIA_EXT = [...AUDIO_EXT, ...VIDEO_EXT];

// ── Host services (toast / navigate / translate) ────────────────────────────
//
// The shim needs three things from whatever shell sits on top of it: a way to
// SAY something went wrong, a way to NAVIGATE (the `?goto=` hook), and a way to
// TRANSLATE the copy for the first. They used to be three hard imports of the
// legacy renderer's own `ui/toast` + `ui/navigate`, which was fine while there
// was one shell and wrong the moment there were two — so S0 put them behind a
// slot with those modules as the defaults.
//
// Fase B deleted the modules with the shell. The defaults are now what is
// TRUE before a host installs its own surfaces: there is no toast stack and no
// router yet, so the only honest thing to do with a message is put it in the
// console, and the only honest thing to do with a navigation is decline it
// loudly. `app/main.tsx` calls `setShimNotifier` as its second act — before
// anything can invoke — so in the shipped app these are unreachable. They exist
// for the window between this module evaluating (which arms the one-shot
// settings migration) and that call, and for a browser boot with no host at
// all; a default that threw, or one that reached for a `document` that has no
// toast root, would turn that window into a crash.
//
// `t` is real, because i18n is not a surface: it is the catalogue, and the
// catalogue is loaded either way. The slot itself (and the partial-override
// merge) is the pure, unit-tested `shim-notifier-core`.
const notifier = createNotifierSlot({
  toast: (kind, msg) => {
    console[kind === "error" ? "error" : "warn"](`[api-shim] ${kind}: ${msg}`);
  },
  navigate: (page, opts) => {
    console.warn(
      `[api-shim] navigate(${page}${opts?.tab ? ":" + opts.tab : ""}) before a host installed a router — ignored`,
    );
  },
  t,
});

/**
 * Install host-provided toast/navigate/translate services. Call it BEFORE the
 * first backend call so an early failure is surfaced by the right shell; a
 * partial override keeps the legacy default for whatever it leaves out, and
 * `null` restores the defaults.
 */
export function setShimNotifier(override: Partial<ShimNotifier> | null): void {
  notifier.set(override);
}

/** A native file/folder picker that returns the chosen path (or null), never
 *  throwing — a denied permission or cancel just yields null. */
async function pickPath(opts: {
  directory?: boolean;
  name?: string;
  extensions?: string[];
  /** Multiple filter groups (first is the default on macOS). Takes precedence
   *  over the single name/extensions pair when present. */
  filters?: { name: string; extensions: string[] }[];
}): Promise<string | null> {
  try {
    const filters =
      opts.filters && opts.filters.length
        ? opts.filters
        : opts.extensions && opts.name
          ? [{ name: opts.name, extensions: opts.extensions }]
          : undefined;
    const res = await openDialog({
      directory: !!opts.directory,
      multiple: false,
      filters,
    });
    return typeof res === "string" ? res : null;
  } catch (e) {
    console.warn("[api-shim] file dialog failed", e);
    return null;
  }
}

/** Convert a local filesystem path to an `asset://` URL WKWebView can load in an
 *  <audio>/<video>/<img> `src`. The OLD Electron renderer used `file://` (and a
 *  custom `media://` protocol), which WKWebView blocks — every editor preview /
 *  mastering playback that set `file://…` was silently dead. The asset protocol
 *  is enabled in tauri.conf with an allow-list scope (the user media roots —
 *  $DOCUMENT/$DOWNLOAD/$VIDEO/$AUDIO/$DESKTOP/$APPDATA/$APPLOCALDATA — plus the
 *  OS temp dir for previews/proxies), so this is the supported path. */
function toAssetUrl(path: string): string {
  return path ? convertFileSrc(path) : "";
}

// ── The fixture seam (E5.1) ─────────────────────────────────────────────────
//
// EVERY `invoke` in this file goes through the wrapper below rather than
// `@tauri-apps/api`'s directly (the real one is imported as `tauriInvoke`), so
// the seam covers `call()`, `editorCall()` AND the ~45 direct call sites with
// one hook instead of 45.
//
// Precedence — an honoured fixture > the real invoke > the caller's fallback —
// and the honour rules live in the pure `fixtures-core`; read its header for the
// reasoning. The two properties that matter here:
//
//   1. With no fixtures installed this is inert. `lookupFixture` misses and the
//      wrapper is a straight pass-through to `tauriInvoke`, so `call()` and
//      everything downstream behave exactly as they did before E5.
//   2. A fixture HIT is not a failure. It short-circuits before `tauriInvoke`,
//      so E2.4's failure ring never sees it and no toast fires. (A fixture that
//      THROWS is a different thing entirely: that rejection travels the normal
//      path and does land in the ring — which is how a test drives the toast.)
//
// Install fixtures before the renderer boots, e.g. from Playwright:
//
//   await page.addInitScript(() => {
//     (window as any).__SUNDAYREC_FIXTURES__ = {
//       app_info: { version: "0.10.0" },             // a value
//       recordings_list: (args) => rowsFor(args),    // or a function of the args
//     };
//   });
//
// Settings are covered like everything else since R4: `getSettings`/`saveSettings`
// are `settings_get`/`settings_save` invokes, so `e2e/harness.ts` seeds them
// with a fixture-backed store — no localStorage involved on either side.
//
const FIXTURE_GATE: FixtureGate = {
  inTauri: isTauri(),
  // Vite inlines this as the literal `false` in a production build, so
  // `FIXTURES_HONORED` below is a constant `false` inside a shipped Tauri
  // bundle — the branch still exists in `fixturesHonored`, but nothing can
  // make it answer `true` here. A shipped SundayRec cannot be driven by
  // fixtures.
  devBuild: !!import.meta.env?.DEV,
  requested: new URLSearchParams(location.search).has(FIXTURE_QUERY_PARAM),
};
const FIXTURES_HONORED = fixturesHonored(FIXTURE_GATE);

/** The installed fixture map, read fresh on every call so a test can swap the
 *  canned answers mid-journey (e.g. "now the list has one more row"). */
function installedFixtures(): FixtureMap | undefined {
  if (!FIXTURES_HONORED) return undefined;
  return (window as unknown as Record<string, unknown>)[FIXTURE_GLOBAL] as
    FixtureMap | undefined;
}

/** `invoke`, with the fixture seam in front of it. Signature-compatible with
 *  `@tauri-apps/api/core`'s, so every existing call site is unchanged. */
function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const found = lookupFixture(installedFixtures(), cmd);
  if (found.hit) {
    // `Promise.resolve` inside try/catch, not an async fn: a fixture that throws
    // SYNCHRONOUSLY must still reject the promise rather than blow up the caller.
    try {
      return Promise.resolve(readFixture(found.value, args) as T);
    } catch (e) {
      return Promise.reject(e);
    }
  }
  return tauriInvoke<T>(cmd, args);
}

/** Every IPC failure this session, bounded, plus the toast rate-limit state.
 *  The policy is the pure, unit-tested `ipc-failures-core`. */
const ipcFailures = createIpcFailureState();

/** Invoke a Tauri command, falling back to `fallback` on any error so the UI
 *  never throws while the backend is partially wired.
 *
 *  E2.4: the fallback stays — the renderer must not die because one command is
 *  missing — but a failure is no longer INVISIBLE. Until now a rejected invoke
 *  produced a `console.warn` and the fallback value, which is exactly what "no
 *  data" looks like: a crashed backend rendered as an empty history list, an
 *  empty device dropdown, a diagnose panel saying everything was fine. (And
 *  until E2.1, a Rust panic WAS a rejected invoke.) So now every failure is
 *  remembered in a bounded ring, and the first of a burst is toasted.
 *
 *  Two guards on the toast:
 *
 *  1. `isTauri()` — in a plain browser (the dev/fixture boot) there is no
 *     backend at all, so every wired command legitimately rejects. Toasting
 *     there would turn opening the page in Chrome into an error storm and
 *     teach everyone to ignore the toast that matters.
 *  2. The dedup/rate-limit in `ipc-failures-core`: one toast per command per
 *     minute, at most three per minute overall. `recording_status` polls ~1×/s
 *     and the preview frame ~4×/s; without this a down backend would stack a
 *     hundred toasts a minute over the UI, which is not surfacing a problem,
 *     it is a second outage.
 *
 *  The ring is filled unconditionally either way — the diagnose panel wants the
 *  pattern, not whichever failure happened to win the rate limit. */
async function call<T>(
  cmd: string,
  args: Record<string, unknown> | undefined,
  fallback: T,
): Promise<T> {
  try {
    return (await invoke<T>(cmd, args)) as T;
  } catch (e) {
    console.warn(`[api-shim] ${cmd} failed → fallback`, e);
    const surface = recordFailure(ipcFailures, cmd, ipcErrText(e), Date.now());
    if (surface && isTauri()) {
      const n = notifier.current();
      n.toast(
        "error",
        `${n.t("error.ipcFailed", "Noe i bakgrunnen svarte ikke, så denne visningen kan være ufullstendig.")} (${cmd})`,
      );
    }
    return fallback;
  }
}

/** Human-readable message from a rejected `invoke`. Tauri serializes our
 *  `AppError` to `{ code, message }` (NOT an `Error` instance), so pull `message`
 *  out of the object; fall back to `code`, a string, or JSON. */
function ipcErrText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object") {
    const o = e as Record<string, unknown>;
    if (typeof o.message === "string" && o.message) return o.message;
    if (typeof o.code === "string" && o.code) return o.code;
  }
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

/** Editor/mastering commands return BARE Rust result structs (e.g. `{outputPath}`),
 *  but the ported Electron consumers expect the old `{ ok, …, error }` envelope —
 *  they all branch on `result.ok` and show `result.error` on failure. Wrap a
 *  success with `ok: true`; on failure surface the REAL reason (`ipcErrText` →
 *  the AppError message, which carries the granular code consumers switch on, e.g.
 *  "no_audio_remaining"/"disk_full"). Previously a failure returned a bare
 *  `{ ok: false }` with NO error, so every friendly editor/export/mastering error
 *  message was dead code ("✕ Feil" with no detail). (IPC-seam audit.) */
async function editorCall<T extends object>(
  cmd: string,
  args: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  try {
    const r = await invoke<T>(cmd, args);
    return { ok: true, ...(r as object) };
  } catch (e) {
    return { ok: false, error: ipcErrText(e) };
  }
}

// Old Electron `on(channel)` → Tauri event name. Channels with no Rust emitter
// (tray-*, update-*, …) fall through to a no-op subscription.
//
// `backend-warning` was the one entry deliberately left OUT by the 2026-08-05
// channel audit: its consumer in pages/home.ts was live, but no src-tauri
// emitter existed under any name, and mapping it to the nearest-looking channel
// would only have manufactured wrong warnings. As of Fase 2 the backend really
// does emit — `crate::notify::warn` on `backend://warning`, from four sources
// (pre-roll gave up, crash recovery skipped a file, the configured device is
// missing, the disk is filling) — so the channel is mapped for real.
const EVENT_MAP: Record<string, string> = {
  "backend-warning": "backend://warning",
  "recording-overlay-start": "recording://started",
  "recording-overlay-stop": "recording://state",
  "recording-finished": "recording://finished",
  "recording-error": "recording://error",
  "recording-warning": "recording://warning",
  // Non-terminal: the stop-on-silence detector's early warning, ahead of the
  // auto-stop timeout. Was emitted by the backend (RecordingEvent {code:
  // "silence_detected", …} from both capture engines) but unmapped here, so
  // users with stop-on-silence enabled got no warning before the auto-stop —
  // consumed in pages/recording.ts alongside recording-warning.
  "recording-silence": "recording://silence",
  "recording-quality": "recording://quality",
  "recording-progress": "recording://progress",
  "recording-levels": "recording://levels",
  "vu-levels": "vu://levels",
  "recording-reconnecting": "recording://reconnecting",
  "recording-reconnected": "recording://reconnected",
  "master-progress": "editor-master-progress",
  "editor-export-progress": "editor://export-progress",
  // Fase 9: the three editor passes that used to run for minutes behind a
  // spinner. All three carry the same `EditorDecodeProgress { fraction }`;
  // separate channels only because they drive separate surfaces (the loading
  // screen, the «Analyser opptak» card, the playback-proxy wait).
  "editor-peaks-progress": "editor://peaks-progress",
  "editor-analysis-progress": "editor://analysis-progress",
  "editor-proxy-progress": "editor://proxy-progress",
};

// Per-event payload ADAPTERS: the Tauri backend emits typed Rust structs whose
// field names (snake_case) / shapes differ from the old Electron IPC payloads
// the ported handlers expect. Each adapter reshapes the payload to what the
// legacy handler reads. (Found by the IPC-seam audit.)
const EVENT_ADAPTERS: Record<string, (p: unknown) => unknown> = {
  // RecordingProgress { bytes_written } → handler reads `bytes`.
  "recording-progress": (p) => {
    const d = (p ?? {}) as { bytes_written?: number };
    return { ...d, bytes: d.bytes_written };
  },
  // RecordingFinished { file_path, has_video } → handler reads `path`. There's no
  // split-restart signal in the Tauri event, so a finished recording always
  // hides the overlay + offers "open in editor" (splitRestart: false).
  "recording-finished": (p) => {
    const d = (p ?? {}) as { file_path?: string };
    return { ...d, path: d.file_path, splitRestart: false };
  },
  // RecordingEvent { code, message } → handler also reads `error` for the
  // localized native-error mapping.
  "recording-error": (p) => {
    const d = (p ?? {}) as { code?: string; message?: string };
    return { ...d, error: d.code };
  },
  // Same payload shape as recording-error — a NON-terminal classified error
  // (the backend reconnect policy retries; the session continues).
  "recording-warning": (p) => {
    const d = (p ?? {}) as { code?: string; message?: string };
    return { ...d, error: d.code };
  },
  // EditorMasterProgress → the mastering panel reads jobId/currentSec/totalSec.
  //
  // The DTO is `#[serde(rename_all = "camelCase")]` (editor/mod.rs), so the
  // payload ALREADY arrives camelCase. This adapter used to read the snake_case
  // names only and spread the results on top — overwriting three perfectly good
  // fields with `undefined` and leaving the bar frozen at 0 % for the entire
  // mastering apply. It is now camelCase-first, with the snake_case read kept
  // purely as a fallback in case an older/renamed emitter ever shows up.
  "master-progress": (p) => {
    const d = (p ?? {}) as {
      jobId?: string;
      currentSec?: number;
      totalSec?: number;
      job_id?: string;
      current_sec?: number;
      total_sec?: number;
    };
    return {
      ...d,
      jobId: d.jobId ?? d.job_id,
      currentSec: d.currentSec ?? d.current_sec,
      totalSec: d.totalSec ?? d.total_sec,
    };
  },
};

// ── Settings (R4: sqlite is the ONE store) ──────────────────────────────────
//
// `settings_get`/`settings_save` carry the FULL settings object in the unified
// (Rust-named) vocabulary — the localStorage blob, the curated
// `backendRecordingSettings` bridge and its 400 lines of "this field too was
// silently re-defaulted" archaeology all died here. What remains renderer-side:
// the one-shot migration below, a loud fallback for a broken store, and the
// launch-at-login OS sync.

// Dev/verification hook (inert in normal use): `?goto=<page>` skips first-run
// onboarding and navigates to the named page after boot, so each screen can be
// screenshotted headlessly. Without the query param this is completely inactive.
//
// Since Fase 7 it also accepts `?goto=<page>:<tab>` for pages with inner tabs —
// `?goto=settings:audio`, `?goto=settings:sharing`. The tab may be written bare
// (`audio`) or fully qualified (`settings-audio`); retired ids from before the
// 7→5 tab fold (`publish`, `notifications`) still work, because
// navigateTo runs them through TAB_ALIASES.
const VERIFY_GOTO = parseGoto(location.search);

// The one-shot localStorage → sqlite migration. Storage side effects live in
// `migrate-legacy-settings.ts` (the only module allowed near the legacy key —
// see settings-store-pin.test.ts); `invoke` is injected so the calls ride the
// fixture seam. `getSettings` awaits this, so the first `settings_get` always
// sees the imported values.
const settingsMigration = migrateLegacySettingsOnce({
  invoke,
  onCorruptBlob: () => {
    if (!isTauri()) return;
    const n = notifier.current();
    n.toast(
      "error",
      n.t(
        "error.settingsMigrationCorrupt",
        "Innstillingene fra forrige versjon kunne ikke leses — appen starter med standardinnstillinger.",
      ),
    );
  },
});

/**
 * `settings_get`, with the two renderer-side responsibilities that remain:
 * wait for the one-shot migration, and make a FAILED read loud. The fallback
 * is `SETTINGS_DEFAULTS` so the UI still renders, but never silently — a
 * broken settings store rendered as "everything is default" is exactly the
 * kind of quiet lie E2.4 exists to end.
 *
 * ## Why "loud" is a BANNER here and not a toast
 *
 * This one failure used to say itself TWICE: a `toast("error", …)` from right
 * here, and the shell's own `hydrate-error` banner (`state/settings.ts` reads
 * the failure ring, `Shell.tsx` renders it) — with the SAME sentence, from the
 * same catalogue key. Two copies of one message is bad on its own; this pair
 * was worse, because an `error` toast has `durationMs: 0` (see `ui/toast.ts`:
 * the one message you cannot afford to miss must not vanish while you look
 * away). So the duplicate sat on top of the shell forever, next to a banner
 * saying the same thing, and dismissing it changed nothing.
 *
 * The house rule decides which one survives: a toast is a RECEIPT for
 * something the user just did, a banner is a STATE that stays wrong until
 * something changes. A settings store that could not be read is a state. So
 * the report belongs to the failure ring — which is already recorded below,
 * and which `hydrateSettings` reads to raise the banner — and this function
 * says nothing itself.
 *
 * That is a narrowing of ONE path, not of the shim's failure toasts in
 * general: `call()`'s E2.4 toast for other commands is untouched, and so is
 * the corrupt-migration toast above (that one has no banner behind it).
 */
async function loadSettingsFromBackend(): Promise<Settings> {
  await settingsMigration;
  let s: Settings;
  try {
    s = await invoke<Settings>("settings_get");
  } catch (e) {
    console.warn("[api-shim] settings_get failed → defaults", e);
    // The ring IS the report: `hydrateSettings` asks it whether this read
    // failed, and the shell's `hydrate-error` banner is the one surface that
    // says so. See the note above for why there is no toast here.
    recordFailure(ipcFailures, "settings_get", ipcErrText(e), Date.now());
    s = { ...SETTINGS_DEFAULTS };
  }
  if (VERIFY_GOTO) {
    // Skip onboarding during verify screenshots (session-only, never saved).
    s = { ...s, onboardingDone: true };
  }
  return s;
}

// Sync the launch-at-login OS login item with the saved setting. The old toggle
// only stored a boolean — scheduled recordings never fired after a reboot because
// the app wasn't actually registered to start. Deduped so a settings_save that
// didn't touch launchAtLogin doesn't re-register the login item.
let lastLaunchAtLogin: boolean | null = null;
async function syncLaunchAtLogin(s: unknown): Promise<void> {
  const enabled = !!(s as { launchAtLogin?: unknown } | null)?.launchAtLogin;
  if (enabled === lastLaunchAtLogin) return;
  lastLaunchAtLogin = enabled;
  try {
    await invoke("set_launch_at_login", { enabled });
  } catch (e) {
    console.warn("[api-shim] set_launch_at_login failed", e);
  }
}

const off = () => {}; // unsubscribe stub

// ── Updater bridge ──────────────────────────────────────────────────────────
// The Tauri updater commands are POLLED (update_status / update_check /
// update_download_install / update_relaunch) rather than event-emitting, so the
// shim synthesizes the Electron-style `update-*` events the renderer listens for
// (see general-page.ts). These channels have no Rust emitter, so `on()` keeps
// their callbacks in this local registry and `checkForUpdates`/`installUpdate`
// drive them.
type UpdateStatus =
  | { phase: "idle" | "checking" | "upToDate" }
  | { phase: "available"; version: string }
  | { phase: "downloading"; version: string; percent: number }
  | { phase: "readyToInstall"; version: string }
  | { phase: "error"; message?: string };
const LOCAL_CHANNELS = new Set([
  "update-checking",
  "update-available",
  "update-not-available",
  "update-download-progress",
  "update-downloaded",
  "update-restarting",
  "update-error",
]);
const localListeners: Record<string, Array<(p: unknown) => void>> = {};
function emitLocal(channel: string, payload?: unknown): void {
  for (const fn of localListeners[channel] ?? []) {
    try {
      fn(payload);
    } catch {
      /* a listener error must not break the updater flow */
    }
  }
}

// ── History adapter: Rust RecordingRow → the old renderer's RecordingEntry ───
type RecordingRow = {
  id: string;
  file_path: string;
  device_name: string | null;
  started_at: number;
  duration_ms: number | null;
  byte_size: number | null;
  created_at: number;
  note: string | null;
};

// Maps the old renderer's `timestamp` key (created_at) back to the Rust row id,
// so deleteHistoryEntry(timestamp) can call recordings_delete(id).
const historyIdByTs = new Map<number, string>();

const basename = (p: string): string => p.split(/[\\/]/).pop() || p;

/** Seconds → the old "Xt Ym" / "Ym" duration string the history table parses. */
function fmtDurXtYm(sec: number): string {
  const totalMin = Math.round(sec / 60);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  return h > 0 ? `${h}t ${m}m` : `${m}m`;
}

function rowToEntry(r: RecordingRow): Record<string, unknown> {
  const path = r.file_path ?? "";
  const filename = basename(path);
  const durationSec =
    r.duration_ms != null ? Math.round(r.duration_ms / 1000) : 0;
  const ts = r.created_at ?? r.started_at ?? 0;
  if (r.id) historyIdByTs.set(ts, r.id);
  return {
    timestamp: ts,
    // `started_at` UNTOUCHED alongside `timestamp`, which is `created_at ??
    // started_at` — the moment the ROW was written, i.e. when a finished
    // service ENDED. The old table only showed a date, so the difference never
    // showed; the new Bibliotek puts the clock in the row's title, where an
    // hour's drift is the difference between «11:00» and «12:05». Additive:
    // every existing consumer reads the fields it always did.
    startedAt: r.started_at,
    date: new Date(ts).toISOString(),
    startTime: "",
    path,
    filename,
    name: filename,
    status: "ok", // recordings_list only holds completed recordings
    durationSec,
    duration: fmtDurXtYm(durationSec),
    sizeBytes: r.byte_size ?? null,
    fileSizeBytes: r.byte_size ?? null,
    note: r.note ?? undefined,
  };
}

/**
 * `listen()` failed for a channel — say so ONCE, then stay quiet.
 *
 * `listen` from `@tauri-apps/api/event` reaches `__TAURI_INTERNALS__` DIRECTLY,
 * so OUTSIDE Tauri (a plain `vite --mode app` browser, or any page without the
 * e2e harness) every subscription rejects. Until now the `.then()` carried no
 * `.catch`, which made each one an unhandled promise rejection: four of them on
 * boot, in red, in a console people are supposed to be reading for real
 * problems — and, since `app/state/global-error.ts` listens for
 * `unhandledrejection`, a shell that reported a global error before it had
 * finished waking up.
 *
 * Inside Tauri `listen` does not reject, so this changes NOTHING about the
 * shipped app. The only visible difference is that a browser boot stops
 * shouting, and that `window.api.on()` keeps its promise either way: it always
 * returns an unsubscribe that is safe to call.
 *
 * Once per CHANNEL, not once per call: a component that mounts and unmounts
 * twenty times would otherwise log twenty identical lines and bury the one
 * channel that is genuinely missing an emitter.
 */
const listenFailedChannels = new Set<string>();
function warnListenFailedOnce(channel: string, err: unknown): void {
  if (listenFailedChannels.has(channel)) return;
  listenFailedChannels.add(channel);
  // `warn`, not `error`: without a Tauri backend this is the EXPECTED state,
  // not a failure — the same call `status/next-recording.ts` already makes.
  console.warn(
    `[api-shim] listen("${channel}") feilet — ingen hendelser kommer fram`,
    err,
  );
}

/** Test-only: forget which channels have already warned. */
export function __resetListenWarnings(): void {
  listenFailedChannels.clear();
}

const api: Record<string, unknown> = {
  // ── Observability (E2.4) ─────────────────────────────────────────────────
  // "Siste IPC-feil" for the diagnose surface. Synchronous and local: this is
  // renderer-side memory, not a backend call, so it still answers when the
  // backend is the thing that is broken — which is the only time it matters.
  getRecentIpcFailures: (): IpcFailure[] => recentFailures(ipcFailures),

  // ── Settings (R4: sqlite is the ONE store) ──────────────────────────────
  getSettings: async () => loadSettingsFromBackend(),
  // WRITE — the FULL object crosses in one vocabulary; nothing is curated, so
  // nothing can be silently re-defaulted (the #113/#115 class ends here, not
  // per-field). A rejected `settings_save` travels to the caller (R3-B): the
  // debounced saver resolves `false` and the «Lagret ✓» chip stays honest.
  saveSettings: async (s: unknown) => {
    await invoke("settings_save", { settings: s });
    // Wake the scheduler so new/changed slots take effect immediately
    // (settings_save alone doesn't recompute the schedule).
    void invoke("scheduler_reschedule").catch(() => {});
    // Register/remove the OS login item to match the toggle.
    void syncLaunchAtLogin(s);
    return true;
  },
  // ── Settings profile (F1.3, wired in R4) ────────────────────────────────
  // Export/import the whole (validated) settings object as a JSON file. The
  // native dialogs are the path authorisation; the backend re-checks with its
  // UserChosenWrite/Read path policies. WRITES both — rejections travel
  // (R3-B) so the card can say what actually went wrong.
  settingsExportToFile: async (path: string) =>
    invoke<void>("settings_export_to_file", { path }),
  // Returns the stored (merged + validated) settings so the caller can
  // rehydrate the UI without a second round-trip.
  settingsImportFromFile: async (path: string) =>
    invoke<Settings>("settings_import_from_file", { path }),
  /** JSON-profile open picker for the import button. Cancel → null. */
  pickSettingsFile: async () =>
    pickPath({
      filters: [
        { name: "Innstillingsprofil (JSON)", extensions: ["json"] },
        { name: "Alle filer", extensions: ["*"] },
      ],
    }),
  // ── Schedule / next recording ───────────────────────────────────────────
  // scheduler_status → { next: ISO string | null }; old getNextRecording returns
  // { date } | null.
  getNextRecording: async () => {
    const s = await call<{ next: string | null }>(
      "scheduler_status",
      undefined,
      {
        next: null,
      },
    );
    return s.next ? { date: s.next } : null;
  },

  // ── History (recordings_list → RecordingEntry[]) ─────────────────────────
  //
  // A trashed recording keeps its history row on purpose (see
  // `src-tauri/src/trash/mod.rs`: the row is what makes a restore give back the
  // note and duration). Filtering it out HERE — rather than in
  // one of the three renderer consumers — is what keeps Historikk, the unified
  // search and the home page's «Siste 5» from disagreeing about whether a
  // recording exists.
  getHistory: async () => {
    historyIdByTs.clear();
    const rows = await call<RecordingRow[]>("recordings_list", undefined, []);
    const trashed = new Set(
      (await call<TrashEntry[]>("trash_list", undefined, [])).map(
        (e) => e.originalPath,
      ),
    );
    return rows.filter((r) => !trashed.has(r.file_path)).map(rowToEntry);
  },

  // ── Papirkurv ────────────────────────────────────────────────────────────
  // These deliberately do NOT swallow their errors into a fallback: a delete
  // that reports success while the file is still there, or an «Angre» that
  // quietly does nothing, is worse than an error message.
  // Retensjonspasset (owner decision 2026-08-31): move recordings older than
  // `autoDeleteDays` into the papirkurv. `call`, not `invoke`: the pass runs
  // unasked at boot and on a timer, so a failure must degrade to "nothing
  // moved" (the next tick tries again) rather than toast an error at a
  // volunteer who did nothing. `disabled: true` is the honest fallback — a
  // pass that did not run must not read as "retention ran, nothing was due".
  recordingsPrune: async () =>
    call<PruneSummary>("recordings_prune", undefined, {
      moved: 0,
      disabled: true,
    }),
  trashMove: async (paths: string[]) =>
    invoke<TrashEntry[]>("trash_move", { paths }),
  trashList: async () => call<TrashEntry[]>("trash_list", undefined, []),
  trashRestore: async (id: string) =>
    invoke<TrashEntry>("trash_restore", { id }),
  trashPurge: async (ids: string[]) => invoke<number>("trash_purge", { ids }),
  deleteHistoryEntry: async (ts: number) => {
    const id = historyIdByTs.get(ts);
    if (!id) return false;
    return call("recordings_delete", { id }, false).then(() => true);
  },

  // ── Disk / recording ────────────────────────────────────────────────────
  // get_disk_space returns { freeBytes } (camelCase) — exactly what home.ts reads.
  getDiskSpace: async () =>
    call("get_disk_space", undefined, { freeBytes: null, totalBytes: null }),
  // Recording: the old renderer builds a full (old-shape) RecordingOpts, but the
  // Rust recorder wants its own RecordingOpts. plan_recording_opts builds the
  // correct one from the persisted settings (the same sqlite store the renderer
  // reads/writes since R4); we only forward customName/maxMinutes/video here.
  startRecordingNow: async (opts: unknown) => {
    const o = (opts ?? {}) as {
      customName?: string;
      maxMinutes?: number;
      videoEnabled?: boolean;
    };
    try {
      const planned = await invoke("plan_recording_opts", {
        customName: o.customName || null,
        maxMinutes: o.maxMinutes ?? null,
        video: !!o.videoEnabled,
      });
      await invoke("start_recording", { opts: planned });
      return { ok: true };
    } catch (e) {
      // AppError serializes to {code,message} (NOT an Error), so the old
      // `String(e)` produced "[object Object]". Surface the message — it carries
      // the granular code `translateNativeError` localizes (e.g. "no_save_folder").
      return { ok: false, error: ipcErrText(e) };
    }
  },
  // WRITE — bare invoke, rejection travels (R3-B house rule: a write that
  // fails must REJECT, never answer a fabricated success). The old
  // `call(…, true)` turned a FAILED stop
  // into `true`: recording.ts then waited politely for a terminal event that
  // was never coming instead of running its own teardown catch.
  stopRecordingNow: async () =>
    invoke("stop_recording", undefined).then(() => true),
  // ── Auto-stop, owned by the recorder ───────────────────────────────────
  //
  // The overlay has always SHOWN the deadline («Stopper av seg selv om 12:04»)
  // and never been able to move it: the two commands were registered in Rust,
  // classified as unreachable in the reachability baseline, and had no door.
  // `manualMaxMinutes` defaults to 0, so it only bit a rig that had turned the
  // safety net ON — and then it bit in the middle of the service.
  //
  // WRITES, so rejection travels (same house rule as `stopRecordingNow`): the
  // deadline the UI draws comes back from the engine on `recording://state`,
  // and a fabricated success here would leave the countdown ticking towards a
  // stop the user believes they cancelled.
  recordingExtendAutostop: async (minutes: number) =>
    invoke<void>("recording_extend_autostop", { minutes }),
  recordingCancelAutostop: async () =>
    invoke<void>("recording_cancel_autostop", undefined),
  // ── The camera picture DURING a recording ──────────────────────────────
  //
  // The engine has written this file since v0.11 (`recorder/engine.rs` —
  // ONE fixed path, overwritten ~12×/s) and the command to read it back has
  // been registered all along. Nothing called it: the door was missing on this
  // side, so `recording_preview_frame` sat in the reachability baseline's
  // `unreachable` list and the overlay showed a CHIP naming the camera instead
  // of the camera. A chip looks identical whether the lens cap is on
  // (docs/SMOKE-TEST.md §"The live camera picture").
  //
  // `null` is a normal answer, not a failure: it means the engine has not
  // written a frame yet (the first one lands a moment after the recording
  // starts). The caller's placeholder stays up — see
  // ui/CameraPreview/PolledCameraPreview.tsx.
  //
  // ⚠️ NOT through `call()`, and this is the one place that is right.
  //
  // The poll runs at 12 Hz for the whole service. `call()` appends every
  // failure to the 50-entry ring unconditionally (the rate limit is only on
  // the TOAST), so a backend that stopped answering would overwrite the entire
  // diagnostic history in four seconds — erasing the record of the very
  // recording that is going wrong, plus a `console.warn` twelve times a
  // second. A lost preview frame is cosmetic; the honest surface for "the
  // camera is dead" is the frame itself never leaving «Starter kamera…»,
  // which is what the person at the machine actually looks at.
  recordingPreviewFrame: async () => {
    try {
      return (await invoke<string | null>("recording_preview_frame")) ?? null;
    } catch {
      return null;
    }
  },
  // ── Pre-roll rolling buffer ────────────────────────────────────────────
  // `start_recording` has always harvested a pre-roll clip, but nothing ever
  // started the loop that produces one — so `preRollSeconds` captured nothing.
  // `preroll_start` answers `false` when the BACKEND's settings say pre-roll is
  // off or no device matched, which is what the Home chip reports (never a
  // claim that the buffer is running when it isn't). See preroll-lifecycle.ts
  // for why this is behind an opt-in.
  prerollStart: async () => call<boolean>("preroll_start", undefined, false),
  prerollStop: async () => {
    try {
      await invoke("preroll_stop");
    } catch (e) {
      console.warn("[api-shim] preroll_stop failed", e);
    }
  },
  prerollStatus: async () =>
    call<import("../../legacy/bindings/PrerollStatus").PrerollStatus>(
      "preroll_status",
      undefined,
      {
        active: false,
        engine: "native",
        channels: 0,
      },
    ),
  // Engine-side VU metering: starts the cpal stream on the device (negotiated
  // FULL channel count — a Qu-5's 32, not getUserMedia's 2) and streams
  // `vu-levels` events (~30/s, one peak+RMS entry per native channel) until
  // stopVu. Returns the negotiated channel count — the channel grid's width.
  startVu: async (deviceName: string | null) =>
    invoke<number>("start_vu", { deviceName }),
  stopVu: async () => invoke<void>("stop_vu"),
  // run_preflight returns Vec<PreflightFinding> directly; old code reads { findings }.
  runPreflight: async () => ({
    findings: await call<unknown[]>("run_preflight", undefined, []),
  }),

  // ── File dialogs / shell (Tauri dialog + opener plugins) ────────────────
  pickFolder: async () => pickPath({ directory: true }),
  openFolder: async (p: string) => {
    try {
      await openPath(p);
      return true;
    } catch {
      return false;
    }
  },
  revealFile: async (p: string) => {
    try {
      await revealItemInDir(p);
      return true;
    } catch {
      return false;
    }
  },

  // ── Email ───────────────────────────────────────────────────────────────
  //
  // `testEmail` was an `async () => ({ ok: false })` stub: every click produced
  // a fabricated "sending failed" no matter what the user had configured. The
  // command exists and is registered (commands/email.rs), so it is wired — and
  // the panel asks `emailStatus` FIRST and disables the button when there is no
  // send path, instead of inventing a failure. `email_send_test` needs
  // `--features email` and returns a clear `feature_disabled` error otherwise,
  // which `emailStatus.featureBuilt` predicts so we never provoke it.
  emailStatus: async () =>
    call<{ featureBuilt: boolean }>("email_status", undefined, {
      featureBuilt: false,
    }),
  testEmail: async (params: {
    recipient: string;
    language?: string;
    host?: string;
    port?: number;
    user?: string;
    pass?: string;
    from?: string;
  }) => {
    try {
      await invoke("email_send_test", {
        recipient: params.recipient,
        language: params.language,
        host: params.host,
        port: params.port,
        user: params.user,
        pass: params.pass,
        from: params.from,
      });
      return { ok: true };
    } catch (e) {
      return { ok: false, error: ipcErrText(e) };
    }
  },

  // The keychain write path — the SMTP password's ONLY home (it is not a
  // `Settings` field, so it can never ride a settings save into the store; the
  // Electron-era cleartext copies were purged in E1.6 and the whole legacy blob
  // is removed by the R4 migration). `email_set_smtp_password` puts it in the
  // OS keychain; passing
  // undefined/"" clears it. Resolves true when a password is now stored.
  // NOT wrapped in `call`: a keychain write that fails must be visible to the
  // caller (it shows an error toast), not silently swallowed into `false`.
  emailSetSmtpPassword: async (password?: string) =>
    (await invoke<boolean>("email_set_smtp_password", {
      password: password && password.length > 0 ? password : null,
    })) as boolean,
  // Wipe the stored password. `email_set_smtp_password(null)` reaches the same
  // `secrets::delete`, and that IS what «Fjern» used to call — which left the
  // dedicated command dark and the intent ambiguous at the seam: a clear read
  // as "a save of nothing", so a keychain failure surfaced under the word
  // «lagret» and the set-path's blank-means-clear branch became the only
  // exercised way to remove a credential. Now the button says what it does.
  // NOT wrapped in `call` — same reason as the write: a keychain delete that
  // fails must reach the caller's error toast, never a silent `false`.
  emailClearSmtpPassword: async () =>
    (await invoke<boolean>("email_clear_smtp_password")) as boolean,
  // Whether a password is stored — drives the "(lagret)" state. The secret
  // itself never crosses into the webview.
  emailHasSmtpPassword: async () =>
    call<boolean>("email_has_smtp_password", undefined, false),

  // ── App / updates ───────────────────────────────────────────────────────
  getAppVersion: async () =>
    (await call<{ version?: string }>("app_info", undefined, {})).version ??
    "—",
  // The menubar tray renders its labels in Rust. Since R4 the backend COULD
  // read `settings.language` from sqlite itself, but the tray must also follow
  // a locale change the moment it happens (and the "follow the OS" null case
  // is resolved renderer-side), so i18n.ts keeps pushing the effective code
  // here on every locale load. Best-effort — a build without the `tray`
  // feature answers with a no-op.
  traySetLanguage: async (code: string) => {
    try {
      await invoke("tray_set_language", { code });
    } catch (e) {
      console.debug("[api-shim] tray_set_language unavailable", e);
    }
  },
  checkForUpdates: async () => {
    emitLocal("update-checking");
    try {
      const st = await invoke<UpdateStatus>("update_check");
      if (st.phase === "available") {
        emitLocal("update-available", { version: st.version });
        return { available: true, version: st.version };
      }
      if (st.phase === "error") {
        emitLocal("update-error", st.message ?? "error");
        return { available: false };
      }
      emitLocal("update-not-available");
      return { available: false };
    } catch (e) {
      emitLocal("update-error", String(e));
      return { available: false };
    }
  },
  installUpdate: async () => {
    // Ask the backend to relaunch, with a visible state and a dead-man's
    // switch: if this process is still alive N seconds after asking, the
    // restart did NOT happen — surface that instead of silence (the recurring
    // 0.4.2→0.4.4 failure mode was precisely a no-op restart button). On a
    // SUCCESSFUL relaunch the process dies and the timer never fires.
    const requestRelaunch = async (): Promise<boolean> => {
      emitLocal("update-restarting");
      const deadMans = setTimeout(
        () => emitLocal("update-error", "restart_failed"),
        6000,
      );
      try {
        await invoke("update_relaunch");
        // macOS: the backend arms the `open`-helper and exits — this promise
        // normally never resolves. If it does, keep the dead-man's switch
        // armed; reaching the timeout means we are provably still running.
        return true;
      } catch (e) {
        clearTimeout(deadMans);
        emitLocal("update-error", String(e));
        return false;
      }
    };
    let timer: ReturnType<typeof setInterval> | undefined;
    try {
      // "Restart & install" after a completed download must RELAUNCH, not
      // re-enter the download pipeline. (The 0.4.2 bug: this always re-ran
      // download_and_install, whose re-check could fail — e.g. the pre-release
      // /releases/latest 404 — and every non-ready outcome returned silently,
      // so clicking the restart button did nothing visible.)
      const cur = await invoke<UpdateStatus>("update_status").catch(() => null);
      if (cur?.phase === "readyToInstall") {
        return await requestRelaunch();
      }
      // Poll the engine status for download progress while the install runs.
      timer = setInterval(() => {
        void invoke<UpdateStatus>("update_status")
          .then((st) => {
            if (st.phase === "downloading")
              emitLocal("update-download-progress", { percent: st.percent });
            else if (st.phase === "readyToInstall")
              emitLocal("update-downloaded", { version: st.version });
          })
          .catch(() => {});
      }, 400);
      const st = await invoke<UpdateStatus>("update_download_install");
      clearInterval(timer);
      if (st.phase === "readyToInstall") {
        emitLocal("update-downloaded", { version: st.version });
        return await requestRelaunch();
      }
      if (st.phase === "upToDate") {
        emitLocal("update-not-available");
        return false;
      }
      // Anything else (error / a phase we don't expect here) is a FAILED
      // install attempt — surface it instead of returning a silent success.
      emitLocal(
        "update-error",
        ("message" in st ? st.message : null) ??
          `unexpected update phase: ${st.phase}`,
      );
      return false;
    } catch (e) {
      if (timer !== undefined) clearInterval(timer);
      emitLocal("update-error", String(e));
      return false;
    }
  },
  logsReveal: async () => {
    try {
      await invoke("logs_reveal");
      return true;
    } catch (e) {
      console.warn("[api-shim] logs_reveal failed", e);
      return false;
    }
  },
  // Clamped server-side to logfile::TAIL_MAX_BYTES (512 KB) no matter what is
  // asked for. Empty string is a valid answer ("nothing logged yet"), so this
  // goes through `call()` with an empty-string fallback rather than throwing.
  logsTail: async (maxBytes: number) =>
    call<string>("logs_tail", { maxBytes }, ""),

  // ── Telemetry (E3.6) — opt-in, anonymous, off by default ────────────────
  // Every command here is documented in src-tauri/src/commands/telemetry.rs;
  // none of them takes or returns a path, a name, or anything else that could
  // identify a person, a church or a recording. The rest of the surface
  // (preview payload, queue status, delete-my-data) is wired in E3.7.
  telemetryConsentGet: async () =>
    call<import("../../legacy/bindings/TelemetryConsent").TelemetryConsent>(
      "telemetry_consent_get",
      undefined,
      // The same "absent means no" default the backend's own state machine
      // uses (crates/sundayrec-core/telemetry/consent.rs) — a failed read
      // must never look like an active grant. `version`/`currentVersion` are
      // 0 rather than a guessed real number: this fallback only fires when
      // the IPC itself is broken, and it has no business pretending to know
      // the live schema version.
      {
        status: "never-asked",
        version: 0,
        decidedAt: null,
        currentVersion: 0,
        needsPrompt: true,
        active: false,
      },
    ),
  // `null` on a real IPC failure — NEVER a fabricated TelemetryConsent. The
  // whole point of "ask once" is that a lost answer has to be asked again, so
  // a caller that swallowed this into a fake "recorded" state could tell the
  // user their choice was saved when it was not.
  telemetryConsentSet: async (granted: boolean) => {
    try {
      return await invoke<
        import("../../legacy/bindings/TelemetryConsent").TelemetryConsent
      >("telemetry_consent_set", { granted });
    } catch (e) {
      console.warn("[api-shim] telemetry_consent_set failed", e);
      return null;
    }
  },

  // ── Telemetry (E3.7) — the settings-panel surface: preview, queue, delete ─
  // "Vis hva som sendes" — the REAL next payload as pretty JSON. `null` ONLY
  // on a genuine IPC failure, never a fabricated payload: telemetry_preview_
  // payload's own doc comment (src-tauri/src/commands/telemetry.rs) is
  // explicit that a mock here would be a promise about code that never runs,
  // which is worse than showing nothing.
  telemetryPreviewPayload: async () => {
    try {
      return await invoke<
        import("../../legacy/bindings/TelemetryPreview").TelemetryPreview
      >("telemetry_preview_payload");
    } catch (e) {
      console.warn("[api-shim] telemetry_preview_payload failed", e);
      return null;
    }
  },
  // "Slett mine data", the local half: retires the install id. `false` only
  // on a real failure — the caller must not claim success it cannot back up.
  telemetryRegenerateInstallId: async () => {
    try {
      await invoke("telemetry_regenerate_install_id");
      return true;
    } catch (e) {
      console.warn("[api-shim] telemetry_regenerate_install_id failed", e);
      return false;
    }
  },

  // ── The e-mail relay (A2) ───────────────────────────────────────────────
  //
  // Five doors for five commands that are registered but not yet used by any
  // page — A5 builds the panel on top of them. They are here NOW rather than
  // with that panel because of what the alternative costs: the reachability
  // gate would otherwise record five newly-registered commands as unreachable
  // and want each one classified as deliberately dark, which is a claim
  // nobody would mean. A thin, typed, honest door is the truthful version of
  // "this is wired, the screen comes next" — and it is what A5 will call.
  //
  // The fallbacks are pessimistic on purpose, exactly as the telemetry block
  // above argues: a failed read must never look like a working subscription.
  // `relayStatus` falls back to "no endpoint, nothing enrolled", which renders
  // as the panel's own empty state.
  relayStatus: async () =>
    call<
      import("../../legacy/bindings/RelaySubscriptionStatus").RelaySubscriptionStatus
    >("relay_status", undefined, {
      endpointBuilt: false,
      state: null,
      address: null,
      enrolledAt: null,
      confirmedAt: null,
      queued: 0,
    }),
  // The three mutations are NOT wrapped in `call`: each is a button press with
  // a granular error the panel has to show (`relay_invalid_address`,
  // `relay_no_endpoint`, `relay_not_confirmed` — all extractable by
  // `errorCode()`). Swallowing one into a fallback status would tell the user
  // their address was accepted when it was refused.
  relaySubscribe: async (address: string) =>
    await invoke<
      import("../../legacy/bindings/RelaySubscriptionStatus").RelaySubscriptionStatus
    >("relay_subscribe", { address }),
  relayResend: async () =>
    await invoke<
      import("../../legacy/bindings/RelaySubscriptionStatus").RelaySubscriptionStatus
    >("relay_resend"),
  relayUnsubscribe: async () =>
    await invoke<
      import("../../legacy/bindings/RelaySubscriptionStatus").RelaySubscriptionStatus
    >("relay_unsubscribe"),
  // Shaped like `testEmail` above, and for the same reason: the button reports
  // its own outcome inline rather than throwing at the page.
  relaySendTest: async () => {
    try {
      await invoke("relay_send_test");
      return { ok: true };
    } catch (e) {
      return { ok: false, error: ipcErrText(e) };
    }
  },

  // ── Health probes ───────────────────────────────────────────────────────
  // Two commands that existed since the port and were never called from
  // anywhere. `media_permissions` is the one that matters: a denied microphone
  // makes the device open fail generically and avfoundation emit "Input/output
  // error", so the user was told the device was MISSING when macOS was simply
  // refusing it. AVFoundation knew all along.
  mediaPermissions: async () =>
    call<import("../../legacy/bindings/MediaPermissions").MediaPermissions>(
      "media_permissions",
      undefined,
      { camera: "unknown", microphone: "unknown" },
    ),
  ffmpegHealth: async () =>
    call<import("../../legacy/bindings/FfmpegHealth").FfmpegHealth>(
      "ffmpeg_health",
      undefined,
      {
        available: true, // unknown ⇒ don't manufacture an alarm
        version: null,
        path: "",
      },
    ),

  // ── Diagnose (V1/PR2) ───────────────────────────────────────────────────
  //
  // The three doors fase B closed when the Diagnose modal went. The commands
  // and their Rust tests never moved; only the surface did, and V1 built it
  // back as a row on Avansert (`pages/setup/advanced/DiagnoseRow.tsx`).
  //
  // ⚠️ These do NOT go through `call()`. Its fallback is the right answer for a
  // list that can be empty — an empty recordings list is a true statement about
  // a machine with no recordings — but a FABRICATED diagnostics report is not:
  // "no findings" is precisely the sentence a volunteer would read as "nothing
  // is wrong". A rejected invoke has to reach the row so it can say the
  // diagnosis did not run. Same reasoning for the test recording: inventing an
  // `ok: false` would name a cause the backend never gave.
  //
  // The IPC failure ring is unaffected — it is fed by every OTHER command's
  // `call()`, and it is that pattern the report renders.
  runDiagnostics: async () =>
    invoke<import("../../legacy/bindings/DiagnosticsReport").DiagnosticsReport>(
      "run_diagnostics",
    ),
  // The purpose-built audio probe: one enumeration → the input names the panel
  // actually asks about. `call()` here (unlike the two above) because an empty
  // name list IS the answer the rows want — "0 devices found" with a cross is
  // the truth when the enumeration fails, and it is said in the row.
  diagnoseAudio: async () =>
    call<import("../../legacy/bindings/AudioDiagnostics").AudioDiagnostics>(
      "diagnose_audio",
      undefined,
      { dshow: [], wasapi: [], wasapiAvailable: false },
    ),
  // ⚠️ NO argument, deliberately. The V1 plan wrote this door as
  // `runTestRecording(deviceName)` — that is the Electron-era shape. The Tauri
  // command takes `db` + `vu` only and reads `settings.device_name` ITSELF
  // (`src-tauri/src/commands/recorder.rs`), so a `deviceName` parameter here
  // would be a lie the type system would then enforce: a caller could pass a
  // device and the test would be run against a different one. The row therefore
  // tests THE CONFIGURED SOURCE, and says so.
  //
  // The command also calls `vu.stop()` before opening the device — macOS gives
  // one client at a time — which is why the row restarts the meter afterwards.
  runTestRecording: async () =>
    invoke<
      import("../../legacy/bindings/TestRecordingResult").TestRecordingResult
    >("run_test_recording"),

  // Whether the OS login item is REALLY registered. The System tab used to show
  // the stored boolean, which drifts the moment a user removes the login item
  // by hand (or a migration/reinstall drops it) — a checkbox claiming scheduled
  // recordings survive a reboot when they don't.
  getLaunchAtLogin: async () =>
    call<boolean>("get_launch_at_login", undefined, false),
  // Trackpad haptics (macOS Force Touch). Infallible by contract on the Rust
  // side; swallow anything here so a haptic can never surface as a UI error.
  hapticPerform: async (pattern: string) => {
    try {
      await invoke("haptic_perform", { pattern });
    } catch {
      /* a missing haptic engine is not a problem worth reporting */
    }
  },

  // ── Audio / video devices ───────────────────────────────────────────────
  // The unified, backend-tagged input list: ASIO devices first (Windows, when a
  // driver is present), then the host's CoreAudio/WASAPI devices, with the
  // WASAPI stereo-pair shadow of an ASIO interface de-duplicated. This is the
  // ONLY device enumeration the renderer has — `enumerateDevices()` needed a
  // getUserMedia grant to reveal labels, and that blink-open made the webview a
  // microphone owner every time a picker rendered (audio/capture.ts).
  listAudioDevices: async () =>
    call<import("../../legacy/bindings/TaggedAudioInput").TaggedAudioInput[]>(
      "list_audio_devices",
      undefined,
      [],
    ),
  // list_devices → { video_inputs: FfmpegDevice[] }; old renderer wants
  // { name, index }[]. FfmpegDevice already carries both fields.
  //
  // ⚠️ REJECTS on a failed read instead of answering with an empty list.
  //
  // The fallback used to be `{}`, so a command that never answered came back as
  // "no cameras" — and the screen then told a volunteer to check a cable that
  // was fine, when the real answer was a camera permission nobody had granted.
  // Two different problems with two different next steps, rendered identically.
  // `null` is the sentinel (the Rust command returns a struct, never null), so
  // the failure still goes through `call()` — E2.4's ring and its rate-limited
  // toast are unchanged — and the CALLER gets to know. `state/devices.ts` is
  // the one caller: it still lands on an empty list, but it also raises
  // `videoDevicesFailed`, which is what the camera preview reads.
  listVideoDevices: async () => {
    const inv = await call<{
      video_inputs?: { name: string; index: number }[];
    } | null>("list_devices", undefined, null);
    if (inv === null) throw new Error("list_devices did not answer");
    return (inv.video_inputs ?? []).map((d) => ({
      name: d.name,
      index: d.index,
    }));
  },
  // Probe what the selected camera can actually capture, to gate the
  // resolution/fps UI. `token` is the device index (avfoundation) or name.
  // Returns null on failure → caller offers everything.
  getCameraCapabilities: async (token: string) =>
    call("get_camera_capabilities", { deviceToken: token }, null),

  // ── Wake from sleep (wake_* commands) ───────────────────────────────────
  // Fallbacks must match the real WakeCapabilities / WakeStatus shapes — the
  // schedule-page reads caps.knownIssues.length / status.expectedWakes.length, so a
  // wrong-shape fallback ({canWake}/{scheduled}) made the reliability card throw and
  // silently disappear whenever the command errored.
  wakeDetectCapabilities: async () =>
    call("wake_capabilities", undefined, {
      platform: "other",
      canWakeFromSleep: false,
      canWakeFromOff: false,
      needsAdmin: false,
      knownIssues: [],
      recommendations: [],
    }),
  // `wake_reschedule` and `wake_verify` came BACK in the settings/status review
  // round. Fase B dropped them with the legacy schedule page — which was the
  // right call for the six-panel diagnostics card that was their only caller,
  // and the wrong one for these two, because between them they are the whole
  // difference between the app SAYING the machine will wake up and the machine
  // actually being armed to:
  //
  //   • `wake_reschedule` is the only user-initiated way to register the OS
  //     wake timers (it may prompt for admin — which is precisely why it can
  //     not live inside the scheduler's own silent pass). Without it the
  //     «Vekk maskinen fra dvale» toggle wrote a boolean nobody could act on.
  //   • `wake_verify` is the only way to know whether the timers are REALLY
  //     there. The hero's «Maskinen vekkes automatisk kl. 10:50» was rendered
  //     off the stored setting alone, i.e. off an intention, not a fact.
  //
  // A FAILED reschedule must report `ok:false` — the Electron shim's old
  // `{ ok:true }` fallback painted a silent failure as success, which is the
  // exact lie this pair exists to end. Same for verify: an unanswered command
  // means "no wakes confirmed", never "all good".
  wakeReschedule: async () =>
    call<WakeResult>("wake_reschedule", undefined, {
      ok: false,
      count: null,
      nextWake: null,
      reason: "error",
      message: null,
      // `null`, og det er ikke en formalitet: `idleReason` svarer på «hvorfor
      // armerte en VELLYKKET reschedule ingenting?». Denne reserven er
      // `ok: false` — kommandoen svarte ikke i det hele tatt — så det finnes
      // ingen tomgangsgrunn å oppgi. Å finne på en her ville vært å forklare
      // bort en feil som en tilstand.
      idleReason: null,
    }),
  wakeVerifyScheduled: async () =>
    call<WakeStatus>("wake_verify", undefined, {
      expectedWakes: [],
      observedWakes: [],
      hasMismatch: false,
      onBattery: null,
      standbyEnabled: null,
    }),

  // ── Editor ──────────────────────────────────────────────────────────────
  // Local path → asset:// URL for <audio>/<video> playback (WKWebView blocks
  // file://). Sync — convertFileSrc returns a string.
  toAssetUrl: (path: string) => toAssetUrl(path),
  editorPickFile: async () =>
    pickPath({
      filters: [
        { name: "Alle støttede medier", extensions: MEDIA_EXT },
        { name: "Lyd", extensions: AUDIO_EXT },
        { name: "Video", extensions: VIDEO_EXT },
        { name: "Alle filer", extensions: ["*"] },
      ],
    }),
  // Map the old export params to EditorExportRequest (outputFormat→format,
  // outputBitrate→bitrate, …; drops mode/processing/metadata). NEEDS LIVE VERIFY.
  editorExportFile: async (params: unknown) => {
    const o = (params ?? {}) as Record<string, unknown>;
    const fmt = (o.outputFormat ?? o.format ?? "mp3") as string;
    const m = (o.metadata ?? {}) as Record<string, unknown>;
    // Title/speaker/description ride along as tags. (v0.15: chapters no
    // longer travel — the chapter UI left with the content cluster.)
    return editorCall("editor_export", {
      request: {
        inputPath: o.inputPath,
        cutRegions: o.cutRegions ?? [],
        duration: o.duration ?? 0,
        // No `container` field: `EditorExportRequest` has never had one, so
        // serde dropped it silently. `format` is the only container the
        // backend reads.
        format: fmt,
        outputFolder: o.outputFolder ?? "",
        bitrate: o.outputBitrate ?? null,
        bitDepth: o.outputBitDepth ?? null,
        masterPreset: o.masterPreset ?? null,
        introPath: o.introPath ?? null,
        outroPath: o.outroPath ?? null,
        gainDb: o.gainDb ?? null,
        title: (m.title as string) || null,
        speaker: (m.speaker as string) || null,
        description: (m.description as string) || null,
        vocalChainPreset: (o.vocalChainPreset as string) || null,
        processing: (o.processing as Record<string, unknown>) ?? null,
        channelRepair: (o.channelRepair as Record<string, unknown>) ?? null,
      },
    });
  },
  // One-click "best result": diagnose + recommended preset bundle.
  editorAutoProcess: async (fp: string) =>
    call("editor_auto_process", { inputPath: fp }, null),
  // Kill the in-flight export's ffmpeg. Returns whether one was running; the
  // export itself then rejects with `cancelled`, which the editor maps to a
  // calm "Eksport avbrutt." (This was a stub returning `true` — the Avbryt
  // button did nothing and a 90-minute render was unkillable.)
  editorCancelExport: async () =>
    call("editor_cancel_export", undefined, false),
  editorPickOutputFolder: async () => pickPath({ directory: true }),
  editorReadCutsDraft: async (fp: string) =>
    call("editor_read_sidecar", { mediaPath: fp, sidecar: "cutsDraft" }, null),
  // The old main wrapped the cut array as { cuts, ts }; preserve that so the
  // loader's `draft.cuts` / age check still work.
  editorSaveCutsDraft: async (fp: string, cuts: unknown) =>
    call(
      "editor_write_sidecar",
      { mediaPath: fp, sidecar: "cutsDraft", value: { cuts, ts: Date.now() } },
      false,
    ).then(() => true),
  editorDeleteCutsDraft: async (fp: string) =>
    call(
      "editor_delete_sidecar",
      { mediaPath: fp, sidecar: "cutsDraft" },
      false,
    ).then(() => true),
  // editor_segments → EditorSegment[]. The consumer (editor/detection.ts) casts
  // the result directly to Suggestion[] and assigns E.suggestions, so return the
  // ARRAY, not a { segments } wrapper (which would make E.suggestions an object).
  // The result is cached in a `<stem>.segments.json` sidecar; `force` (the
  // explicit «Analyser opptak» button) re-runs the analysis instead of reading it.
  editorDetectSegments: async (fp: string, force?: boolean) =>
    call("editor_segments", { inputPath: fp, force: force ?? false }, []),
  // E8 — the sermon dropdown's correction, persisted next to the recording in
  // `<stem>.feedback.json`. Resolves to whether anything was written: picking
  // the block the detector already chose is not a correction. Never throws; a
  // failure to record must not interrupt an edit the user is in the middle of.
  editorRecordSermonPick: async (fp: string, request: unknown) =>
    call("editor_record_sermon_pick", { mediaPath: fp, request }, false),
  // The other half: which of these segments the human's stored correction means
  // (`null` when there is none). Matched on OFFSETS in the backend, because the
  // indices in a stored record mean nothing once detection has run again.
  editorSermonPick: async (fp: string, segments: unknown) =>
    call<number | null>(
      "editor_sermon_pick",
      { mediaPath: fp, segments },
      null,
    ),
  // editor_load_recording → EditorMediaInfo { durationSec, hasVideo, hasAudio, … }.
  // An ffprobe-only probe: it gives the audio loader the authoritative duration
  // WITHOUT reading a byte of media, which is what lets the editor paint a
  // timeline for a multi-GB recording instantly. `null` on any failure.
  editorLoadRecording: async (fp: string) =>
    call<{
      durationSec: number;
      hasVideo: boolean;
      hasAudio: boolean;
      channels: number | null;
      sampleFmt: string | null;
      sampleRate: number | null;
    } | null>("editor_load_recording", { inputPath: fp }, null),
  // editor_allow_asset_path → widens the webview's `asset://` scope to ONE file.
  // The static scope globs cover the standard user folders only; recordings on
  // an external drive/share match none of them and the <audio> src fails with an
  // opaque media error. Call this before pointing any element at a path. Returns
  // false when the grant was refused (guarded path / feature-off) — the caller
  // still tries the element, since paths inside the static scope work regardless.
  editorAllowAssetPath: async (fp: string) => {
    try {
      await invoke("editor_allow_asset_path", { path: fp });
      return true;
    } catch (e) {
      console.warn("[api-shim] editor_allow_asset_path failed", e);
      return false;
    }
  },
  // editor_peaks → { peaks, sampleRate } — the waveform for BOTH the audio and
  // the video loader (playback is always a media element on asset://). Streamed
  // out of ffmpeg 100 peaks/s and cached in a `<stem>.peaks.json` sidecar, so a
  // reopen costs a JSON read instead of a full decode.
  editorExtractAudioPeaks: async (fp: string) =>
    call("editor_peaks", { inputPath: fp }, null),
  // editor_extract_playback_proxy → a seekable stereo AAC .m4a temp file. The
  // LAST-RESORT playback transport: used only when the webview has no decoder
  // for the container, or when the original refused to open. Normal playback
  // streams the ORIGINAL over asset://; this streams the same way, so neither
  // path builds a multi-GB Web-Audio PCM buffer. Returns the proxy path, or null
  // when even the transcode failed (the editor then says playback is
  // unavailable — cuts and export still run on the original).
  editorExtractPlaybackProxy: async (fp: string) =>
    call<string | null>(
      "editor_extract_playback_proxy",
      { inputPath: fp },
      null,
    ),
  // Video export → editor_export with a video container (mp4/mov/mkv) + codec
  // (h264/h265). Maps the renderer params to EditorExportRequest just like
  // editorExportFile (the old raw-passthrough shape didn't match the request).
  editorExportVideo: async (params: unknown) => {
    const o = (params ?? {}) as Record<string, unknown>;
    const m = (o.metadata ?? {}) as Record<string, unknown>;
    const fmt = (o.videoFormat as string) || "mp4";
    return editorCall("editor_export", {
      request: {
        inputPath: o.inputPath,
        cutRegions: o.cutRegions ?? [],
        duration: o.duration ?? 0,
        format: fmt,
        outputFolder: o.outputFolder ?? "",
        bitrate: null,
        bitDepth: null,
        masterPreset: (o.masterPreset as string) || null,
        introPath: o.introPath ?? null,
        outroPath: o.outroPath ?? null,
        // The normalize gain the user set applies to a video export's AUDIO
        // track exactly as it does to an audio export — this used to be
        // hard-coded `null`, so "Normaliser" was silently a no-op for video.
        gainDb: o.gainDb ?? null,
        title: (m.title as string) || null,
        speaker: (m.speaker as string) || null,
        description: (m.description as string) || null,
        vocalChainPreset: (o.vocalChainPreset as string) || null,
        processing: (o.processing as Record<string, unknown>) ?? null,
        channelRepair: (o.channelRepair as Record<string, unknown>) ?? null,
        videoCodec: (o.videoCodec as string) || null,
      },
    });
  },
  // ── Mastering (editor_master_* / editor_mastering_analyze) ──────────────
  // editor_master_preview/apply take a single `request` struct; cancel takes jobId.
  // Mastering commands return bare structs; the consumer expects { ok, … }.
  masterPreview: async (
    inputPath: string,
    presetId: string,
    startSec: number,
    durationSec: number,
  ) =>
    editorCall("editor_master_preview", {
      request: { inputPath, presetId, startSec, durationSec },
    }),

  registerTrustedPath: async () => true,

  // Native "save as" picker (the dialog plugin's counterpart to `pickPath`).
  // A cancel yields null — never throws, same contract as the open pickers.
  pickSavePath: async (opts: {
    defaultPath?: string;
    name?: string;
    extensions?: string[];
  }) => {
    try {
      const res = await saveDialog({
        defaultPath: opts.defaultPath,
        filters:
          opts.extensions && opts.name
            ? [{ name: opts.name, extensions: opts.extensions }]
            : undefined,
      });
      return typeof res === "string" ? res : null;
    } catch (e) {
      console.warn("[api-shim] save dialog failed", e);
      return null;
    }
  },

  // ── Fire-and-forget (Electron ipcRenderer.send) ─────────────────────────

  // ── Event subscriptions ─────────────────────────────────────────────────
  // Map the old Electron channel to its Tauri event and forward the payload.
  // Unknown channels (no Rust emitter yet) return a harmless no-op unsubscribe.
  on: (channel: string, fn: (...args: unknown[]) => void) => {
    // Frontend-synthesized updater channels (no Rust emitter) — keep locally.
    if (LOCAL_CHANNELS.has(channel)) {
      (localListeners[channel] ??= []).push(fn as (p: unknown) => void);
      return () => {
        localListeners[channel] = (localListeners[channel] ?? []).filter(
          (g) => g !== fn,
        );
      };
    }
    const evt = EVENT_MAP[channel];
    if (!evt) return off;
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    const adapt = EVENT_ADAPTERS[channel];
    listen(evt, (e) => fn(adapt ? adapt(e.payload) : e.payload))
      .then((u) => {
        if (cancelled) u();
        else unlisten = u;
      })
      .catch((err) => warnListenFailedOnce(channel, err));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  },
};

(window as unknown as Record<string, unknown>).api = api;

// Keep the OS login item in sync with the persisted launch-at-login flag on
// boot (re-registers if the OS dropped it; idempotent otherwise). Reads the
// backend store — a failed read syncs nothing rather than syncing a guess.
// (The old boot-time settings_save "seed" is gone with the bridge: sqlite IS
// the store now, so there is nothing to push at boot, only to read.)
void (async () => {
  try {
    await settingsMigration;
    const s = await invoke<Settings>("settings_get");
    void syncLaunchAtLogin(s);
  } catch {
    /* no backend (browser tier) or broken store — nothing to sync */
  }
})();

// ── Native drag-drop bridge ───────────────────────────────────────────────
// Tauri intercepts OS file drags (dragDropEnabled defaults to true), so the
// legacy pages' HTML5 dragover/drop handlers never fire — and even if they
// did, Electron's non-standard `File.path` doesn't exist here. Bridge the
// native stream back into the DOM: re-dispatch synthetic DragEvents at the
// drop position with File objects carrying a real `path` property, so the
// editor's load/intro/outro zones work unmodified.
void (async () => {
  try {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    let lastTarget: Element | null = null;

    const dispatch = (
      type: string,
      el: Element | null,
      x: number,
      y: number,
      paths?: string[],
    ): void => {
      if (!el) return;
      const ev = new DragEvent(type, {
        bubbles: true,
        cancelable: true,
        clientX: x,
        clientY: y,
      });
      try {
        const dt = new DataTransfer();
        for (const p of paths ?? []) {
          const name = p.split(/[\\/]/).pop() ?? p;
          const f = new File([], name);
          // The legacy handlers read Electron's non-standard `File.path`.
          Object.defineProperty(f, "path", { value: p });
          dt.items.add(f);
        }
        // DragEvent's init dict ignores dataTransfer in some WebKit builds —
        // defineProperty works everywhere.
        Object.defineProperty(ev, "dataTransfer", { value: dt });
      } catch (e) {
        console.warn("[api-shim] drag-drop dataTransfer synth failed", e);
      }
      el.dispatchEvent(ev);
    };

    await getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload as {
        type: string;
        position?: { x: number; y: number };
        paths?: string[];
      };
      // Native positions are physical pixels; the DOM wants logical.
      const scale = window.devicePixelRatio || 1;
      const x = (payload.position?.x ?? 0) / scale;
      const y = (payload.position?.y ?? 0) / scale;
      if (payload.type === "enter" || payload.type === "over") {
        const el = document.elementFromPoint(x, y);
        if (lastTarget && lastTarget !== el) {
          dispatch("dragleave", lastTarget, x, y);
        }
        lastTarget = el;
        dispatch("dragover", el, x, y);
      } else if (payload.type === "drop") {
        const el = document.elementFromPoint(x, y);
        if (lastTarget && lastTarget !== el) {
          dispatch("dragleave", lastTarget, x, y);
        }
        dispatch("drop", el, x, y, payload.paths);
        lastTarget = null;
      } else {
        // "leave" / cancelled.
        dispatch("dragleave", lastTarget, 0, 0);
        lastTarget = null;
      }
    });
  } catch (e) {
    console.warn("[api-shim] native drag-drop bridge unavailable", e);
  }
})();

// Mark this file as a module (loaded via <script type="module">) so its
// top-level helpers (loadSettings, api, …) stay module-scoped and don't collide
// with the renderer's global declarations. Phase 3 adds real imports here.
export {};

// Verification navigation (only with `?goto=<page>[:<tab>]`): poll until main.ts
// has installed window.showPage, then navigate. Inert without the query param.
if (VERIFY_GOTO) {
  // The page/tab normalisation lives in the pure `goto-core`; what remains here
  // is the boot-time polling, which is DOM/timing and cannot be pure.
  const { page: gotoPage, tab: gotoTab } = VERIFY_GOTO;
  const tryGoto = (): void => {
    // `window.showPage` is declared as always-present in api.d.ts (it is the
    // shell's ONE global), but it is only INSTALLED once main.tsx boots — so
    // the runtime check below still matters even though the type says
    // otherwise. No `any` needed: the declared type is exactly what is called.
    if (typeof window.showPage !== "function") {
      setTimeout(tryGoto, 50);
      return;
    }
    // No highlight pulse: this path exists to produce clean screenshots, and a
    // 4.4 s glow on the card would be in half of them.
    if (gotoTab)
      notifier.current().navigate(gotoPage, { tab: gotoTab, highlight: false });
    else window.showPage(gotoPage);
  };
  setTimeout(tryGoto, 150);
}
