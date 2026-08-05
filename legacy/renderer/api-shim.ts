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
// NOTE: VU metering has THREE separate paths — audio/video device enumeration
// is client-side (Web Audio getUserMedia / enumerateDevices) throughout, but
// don't assume that covers metering too:
//   1. Home / Live / Onboarding meters open their own client-side getUserMedia
//      + Web Audio analyser — no backend involved.
//   2. The channel grid (audio-page) uses the BACKEND cpal VU engine via
//      startVu/stopVu + the `vu-levels` event, because getUserMedia caps a
//      device at 2 channels and the grid needs every channel a digital mixer
//      (e.g. a 32-channel Qu-5) actually exposes.
//   3. The recording overlay's meter is driven by the ACTIVE RECORDING's own
//      `recording://levels` telemetry (see pages/recording.ts) — never a
//      second getUserMedia stream, so the mic keeps exactly one owner for the
//      whole take.

import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  open as openDialog,
  save as saveDialog,
} from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import { navigateTo } from "./ui/navigate";

// Broad, VLC-like accept lists — the bundled ffmpeg demuxes all of these, and
// the loader falls back to a full-fidelity AAC proxy (streamed from disk, same
// as the original) for anything the webview can't decode directly. Keep these in
// sync with the drag-drop sets in editor-page.ts / editor/state.ts.
const AUDIO_EXT = [
  "mp3", "mp1", "mp2", "wav", "flac", "aac", "m4a", "m4b", "m4r", "ogg", "oga",
  "opus", "aiff", "aif", "wma", "mka", "ac3", "eac3", "amr", "3ga", "caf", "wv",
  "tta", "au", "snd", "ape", "dts", "mpc", "ra", "ram", "spx", "gsm",
];
const VIDEO_EXT = [
  "mp4", "mov", "mkv", "m4v", "webm", "avi", "wmv", "ts", "mts", "m2ts", "flv",
  "3gp", "asf", "f4v",
];
const IMAGE_EXT = ["png", "jpg", "jpeg", "webp", "gif"];
// Everything the editor can ingest — audio OR video. The loader probes/decodes
// per file, so the picker should be as accepting as possible.
const MEDIA_EXT = [...AUDIO_EXT, ...VIDEO_EXT];

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

/** Invoke a Tauri command, falling back to `fallback` on any error so the UI
 *  never throws while the backend is partially wired. */
async function call<T>(
  cmd: string,
  args: Record<string, unknown> | undefined,
  fallback: T,
): Promise<T> {
  try {
    return (await invoke<T>(cmd, args)) as T;
  } catch (e) {
    console.warn(`[api-shim] ${cmd} failed → fallback`, e);
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
// (tray-*, update-*, cloud-upload-*, …) fall through to a no-op subscription.
//
// Deliberately NOT mapped (2026-08-05 channel audit): `backend-warning`. Its
// consumer in pages/home.ts is live, but a search of src-tauri turns up no
// emitter for it under any name — mapping it to the nearest-looking channel
// would only manufacture wrong warnings. It stays unmapped until the backend
// actually emits something.
const EVENT_MAP: Record<string, string> = {
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
  "video-preview-frame": "preview://frame",
  // A camera that failed to start / lost its device. The backend's only live
  // camera-failure emitter is the preview module's `preview://error`
  // (media/preview.rs), whose `PreviewError.message` is already user-facing.
  // The consumer (pages/recording.ts) swaps the dead placeholder for "Kamera
  // feilet — opptar kun lyd". Caveat worth knowing: the in-recording preview is
  // a file sink, not this module, so this only fires when a backend preview is
  // actually running (the Direktesending page starts one).
  "video-capture-error": "preview://error",
  "master-progress": "editor-master-progress",
  "whisper-progress": "whisper://progress",
  "whisper-model-progress": "whisper://model-progress",
  "stream-stats": "streaming://stats",
  "editor-export-progress": "editor://export-progress",
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
  // PreviewFrame { data: <base64>, … } → the legacy frame handlers expect raw
  // JPEG bytes (normalizeFrameData). Decode base64 → Uint8Array.
  "video-preview-frame": (p) => {
    const d = p as { data?: string } | undefined;
    if (d && typeof d.data === "string") {
      const bin = atob(d.data);
      const arr = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
      return arr;
    }
    return p;
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

// ── Default settings (mirrors OLD src/main/store.ts `defaults`) ───────────────
const DEFAULT_SETTINGS: Record<string, unknown> = {
  language: null,
  hasLaunched: false,
  deviceId: null,
  deviceName: null,
  deviceChannels: {},
  channels: "stereo",
  sampleRate: 48000,
  sampleRateMode: "auto",
  inputVolume: 100,
  eqBass: 0,
  eqMid: 0,
  eqTreble: 0,
  compEnabled: false,
  compThreshold: -24,
  compRatio: 4,
  compAttack: 10,
  compRelease: 200,
  limiterEnabled: true,
  limiterCeiling: -1,
  format: "mp3",
  bitrate: "256",
  filenamePattern: "date",
  saveFolder: null,
  autoDeleteDays: 0,
  slots: [],
  specialRecordings: [],
  stopOnSilence: false,
  splitMinutes: 0,
  reminderMinutes: 0,
  manualMaxMinutes: 0,
  preRollSeconds: 0,
  prerollEnabled: false,
  launchAtLogin: false,
  showOnStartup: false,
  minimizeToTray: true,
  wakeFromSleep: true,
  protectRecording: true,
  notifyStart: true,
  notifyStop: true,
  emailOnError: false,
  emailAddress: "",
  emailSmtp: "",
  emailSmtpPort: 587,
  emailSmtpUser: "",
  emailSmtpPass: "",
  autoUpdate: true,
  askOpenEditor: true,
  editorIntroPath: undefined,
  editorOutroPath: undefined,
  editorHwEncode: false,
  cloudGoogleDrive: undefined,
  cloudDropbox: undefined,
  cloudOneDrive: undefined,
  churchName: "",
  responsiblePerson: "",
  integrations: { enabled: false },
  activeRecovery: null,
  nextExpectedRecordingISO: null,
  recordingHistory: [],
  wakeFailureHistory: [],
};

const LS_KEY = "sundayrec.settings";

// Dev/verification hook (inert in normal use): `?goto=<page>` skips first-run
// onboarding and navigates to the named page after boot, so each screen can be
// screenshotted headlessly. Without the query param this is completely inactive.
//
// Since Fase 7 it also accepts `?goto=<page>:<tab>` for pages with inner tabs —
// `?goto=settings:audio`, `?goto=settings:sharing`. The tab may be written bare
// (`audio`) or fully qualified (`settings-audio`); retired ids from before the
// 7→5 tab fold (`publish`, `notifications`, `integrations`) still work, because
// navigateTo runs them through TAB_ALIASES.
const VERIFY_GOTO = new URLSearchParams(location.search).get("goto");

function loadSettings(): Record<string, unknown> {
  try {
    const saved = JSON.parse(localStorage.getItem(LS_KEY) || "{}");
    const merged = { ...DEFAULT_SETTINGS, ...saved };
    if (VERIFY_GOTO) {
      merged.hasLaunched = true; // skip onboarding during verify screenshots
      merged.onboardingDone = true;
    }
    return merged;
  } catch {
    return { ...DEFAULT_SETTINGS };
  }
}

// SECURITY (2026-08 audit): the Rust backend has a real keychain slot for the
// SMTP password (secrets::SecretProvider::SmtpPassword, cleared via
// email_clear_smtp_password) but no command to SET it — the port never wired
// a save path there. This function was the ONLY place the raw password
// landed, so every save wrote it to localStorage in cleartext, forever. Strip
// it before persisting; `emailSmtpPass` lives in the in-memory `settings`
// singleton for the current session only, matching the field's own doc
// comment in types/index.ts ("runtime only — always '' in store").
function saveSettingsLocal(s: unknown): boolean {
  try {
    const { emailSmtpPass: _droppedSmtpPass, ...persisted } = (s ?? {}) as Record<string, unknown>;
    localStorage.setItem(LS_KEY, JSON.stringify(persisted));
    return true;
  } catch {
    return false;
  }
}

// The UI's full settings live in localStorage (83 fields, superset of the Rust
// Settings). But the RECORDER reads the backend (sqlite) settings via
// `plan_recording_opts` → `settings::load(db)`, which the UI never wrote to — so
// the user's resolution/format/camera/codec choices NEVER reached an actual
// recording (it used backend DEFAULTS). This pushes the recording-critical subset
// to the backend so manual + scheduled recordings honour the UI. Only a curated
// set of fields with KNOWN-COMPATIBLE types is sent (the Rust enums for
// format/channels are ported 1:1, so the string values match); everything else
// defaults backend-side. Best-effort — a deserialize error leaves the backend
// unchanged (no regression). `settings_save` deserializes with serde(default).
function backendRecordingSettings(s: Record<string, unknown>): Record<string, unknown> {
  // Channel L/R: the recorder reads TOP-LEVEL inputChannelL/R (custom_channel_map_filter
  // records ANY two device channels into a stereo file — e.g. an X32 mixer on ch 16/17),
  // but the audio-page stores the mapping PER DEVICE in deviceChannels[deviceId]. So the
  // recorder never saw it → channel selection was silently ignored (always default 0/1).
  // Translate the SELECTED device's mapping to the top-level fields; clamp 0..31 mirrors
  // the Rust validate(). Default (0,1) is a no-op in custom_channel_map_filter, so this
  // only changes behaviour when the user actually picked non-default channels.
  const deviceChannels = (s.deviceChannels ?? {}) as Record<
    string,
    { channelL?: unknown; channelR?: unknown }
  >;
  const selDeviceId = (s.deviceId as string | null) ?? null;
  const chMap = (selDeviceId && deviceChannels[selDeviceId]) || {};
  const clampCh = (v: unknown): number | null =>
    typeof v === "number" && Number.isInteger(v) ? Math.min(31, Math.max(0, v)) : null;
  return {
    deviceId: s.deviceId ?? null,
    deviceName: s.deviceName ?? null,
    videoEnabled: s.videoEnabled ?? false,
    videoDeviceName: s.videoDeviceName ?? null,
    videoDeviceIndex: s.videoDeviceIndex ?? null,
    videoResolution: s.videoResolution ?? "1080p",
    videoFramerate: s.videoFramerate ?? 30,
    videoContainer: s.videoContainer ?? "mp4",
    videoCodec: s.videoCodec ?? "h264",
    videoEncoder: s.videoEncoder ?? "hardware",
    videoFlip: s.videoFlip ?? false,
    outputMode: s.videoSeparate ? "separate" : "combined",
    keepSeparateAudio: s.videoKeepAudio !== false,
    // Windows escape hatch: force legacy DirectShow audio over cpal (WASAPI/ASIO).
    classicDirectshow: s.classicDirectshow ?? false,
    // Escape hatch: force legacy ffmpeg audio capture over the native engine.
    classicFfmpegAudio: s.classicFfmpegAudio ?? false,
    separateAudioFormat: s.format ?? "wav",
    channels: s.channels ?? "stereo",
    inputChannelL: clampCh(chMap.channelL),
    inputChannelR: clampCh(chMap.channelR),
    // Sample-rate policy → Rust SampleRate enum. The recorder uses this (Auto =
    // native capture, no -ar resampling); the numeric `sampleRate` is client-only.
    // Whitelisted so a stale value can't fail the whole settings_save.
    sampleRateMode: (["auto", "r44100", "r48000", "r96000"] as const).includes(
      s.sampleRateMode as "auto" | "r44100" | "r48000" | "r96000",
    )
      ? (s.sampleRateMode as string)
      : "auto",
    format: s.format ?? "mp3",
    bitrate: String(s.bitrate ?? "256"),
    saveFolder: s.saveFolder ?? null,
    // The filename pattern drives the recorder's output filename (build_opts →
    // build_filename). Omitting it let Rust's #[serde(default)] re-default it to
    // `date` on every settings_save, so a user who picked church/plain/datetime
    // had every recording silently named with the `date` pattern. Whitelisted
    // because a stale/corrupt localStorage value would otherwise fail the WHOLE
    // settings_save (serde rejects an unknown enum), dropping ALL recorder sync.
    filenamePattern: (["date", "church", "plain", "datetime"] as const).includes(
      s.filenamePattern as "date" | "church" | "plain" | "datetime",
    )
      ? (s.filenamePattern as string)
      : "date",
    stopOnSilence: s.stopOnSilence ?? false,
    silenceThreshold: s.silenceThreshold ?? -50,
    silenceTimeoutMinutes: s.silenceTimeoutMinutes ?? 5,
    splitMinutes: s.splitMinutes ?? 0,
    manualMaxMinutes: s.manualMaxMinutes ?? 0,
    preRollSeconds: s.preRollSeconds ?? 0,
    // Wake-from-sleep drives the BACKEND scheduler's OS-wake arming
    // (scheduler/mod.rs reads settings.wake_from_sleep). Must be synced or the
    // Rust `#[serde(default = "default_true")]` re-defaults it to `true` on every
    // settings_save → a user who turns wake OFF could never make it stick and the
    // machine would keep waking for scheduled recordings.
    wakeFromSleep: s.wakeFromSleep ?? true,
    // The weekly schedule + one-off recordings drive the BACKEND scheduler
    // (which couldn't see them while settings lived only in localStorage → no
    // scheduled recording ever fired). SANITISED so a single malformed entry
    // can't fail the whole settings_save (which would also drop the recording
    // settings). Shapes match the Rust ScheduleSlot / SpecialRecording.
    slots: sanitizeSlots(s.slots),
    specialRecordings: sanitizeSpecials(s.specialRecordings),
    // Editor video export: opt into the macOS VideoToolbox hardware encoder.
    // Read by `editor_export`; must be synced or Rust's `#[serde(default)]`
    // re-defaults it to `false` on every settings_save and the toggle never
    // sticks (the same trap `filenamePattern`/`wakeFromSleep` fell into).
    editorHwEncode: s.editorHwEncode ?? false,
  };
}

function sanitizeSlots(v: unknown): Array<Record<string, unknown>> {
  if (!Array.isArray(v)) return [];
  return v
    .filter((sl) => sl && Array.isArray((sl as { days?: unknown }).days))
    .map((sl) => {
      const o = sl as Record<string, unknown>;
      return {
        days: (o.days as unknown[]).filter((d) => Number.isInteger(d)),
        start: typeof o.start === "string" ? o.start : "10:00",
        stop: typeof o.stop === "string" ? o.stop : "12:00",
        max: typeof o.max === "number" ? o.max : null,
      };
    });
}

function sanitizeSpecials(v: unknown): Array<Record<string, unknown>> {
  if (!Array.isArray(v)) return [];
  return v
    .filter((r) => r && typeof (r as { date?: unknown }).date === "string")
    .map((r) => {
      const o = r as Record<string, unknown>;
      return {
        id: typeof o.id === "string" ? o.id : null,
        date: o.date as string,
        name: typeof o.name === "string" ? o.name : "",
        start: typeof o.start === "string" ? o.start : "10:00",
        stop: typeof o.stop === "string" ? o.stop : "12:00",
      };
    });
}

let lastSyncedJson = "";
async function syncBackendRecordingSettings(s: unknown): Promise<void> {
  try {
    const curated = backendRecordingSettings((s ?? {}) as Record<string, unknown>);
    const json = JSON.stringify(curated);
    if (json === lastSyncedJson) return; // nothing changed
    lastSyncedJson = json;
    await invoke("settings_save", { settings: curated });
    // Wake the scheduler supervisor so it picks up new/changed slots immediately
    // (settings_save alone doesn't recompute the schedule).
    try {
      await invoke("scheduler_reschedule");
    } catch {
      /* scheduler reschedule is best-effort */
    }
  } catch (e) {
    console.warn("[api-shim] backend settings sync failed (recording will use defaults)", e);
  }
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

const noop = (): void => {};
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

// Platform from the webview UA (the renderer's init() also checks this).
const platform = navigator.userAgent.toLowerCase().includes("mac")
  ? "darwin"
  : navigator.userAgent.toLowerCase().includes("win")
    ? "win32"
    : "linux";

// Common stub shapes so renderers that read fields/iterate don't throw.
const okFalse = { connected: false, configured: false };
const cloudStatusStub = {
  googleDrive: { connected: false },
  dropbox: { connected: false },
  oneDrive: { connected: false },
};
const streamStatusStub = {
  active: false,
  uptime: 0,
  // Field names match what live-page.ts reads (s.bitrateKbps / s.dropped / s.fps),
  // so idle stats show "0 kbps" / "0" like the old app — not "undefined".
  bitrateKbps: 0,
  fps: 0,
  dropped: 0,
  destinations: [],
};

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

/** The bit of `ReviewQueueEntry` the shim itself needs (picking by id). The
 *  full shape is the renderer's `ReviewQueueEntry` / the ts-rs binding — the
 *  backend already serialises it camelCase, so it passes through untouched. */
type ReviewQueueEntryLike = { id: string };

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
  const durationSec = r.duration_ms != null ? Math.round(r.duration_ms / 1000) : 0;
  const ts = r.created_at ?? r.started_at ?? 0;
  if (r.id) historyIdByTs.set(ts, r.id);
  return {
    timestamp: ts,
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
    cloudUploaded: [],
    cloudUrls: {},
  };
}

const api: Record<string, unknown> = {
  // ── Settings ────────────────────────────────────────────────────────────
  getSettings: async () => loadSettings(),
  saveSettings: async (s: unknown) => {
    const ok = saveSettingsLocal(s);
    void syncBackendRecordingSettings(s); // push recording-critical subset to sqlite
    void syncLaunchAtLogin(s); // register/remove the OS login item to match the toggle
    return ok;
  },
  // ── Schedule / next recording ───────────────────────────────────────────
  // scheduler_status → { next: ISO string | null }; old getNextRecording returns
  // { date } | null.
  getNextRecording: async () => {
    const s = await call<{ next: string | null }>("scheduler_status", undefined, {
      next: null,
    });
    return s.next ? { date: s.next } : null;
  },

  // ── History (recordings_list → RecordingEntry[]) ─────────────────────────
  getHistory: async () => {
    historyIdByTs.clear();
    const rows = await call<RecordingRow[]>("recordings_list", undefined, []);
    return rows.map(rowToEntry);
  },
  deleteHistoryEntry: async (ts: number) => {
    const id = historyIdByTs.get(ts);
    if (!id) return false;
    return call("recordings_delete", { id }, false).then(() => true);
  },
  clearHistory: async () => call("recordings_clear", undefined, false).then(() => true),
  // recordings_prune returns a PruneSummary object; the consumer compares the
  // result to 0 ("Ingen å rydde"), so return the numeric deleted count, not the
  // object (an object is never === 0, so that hint never showed).
  pruneHistory: async () => {
    const r = await call<{ deleted?: number }>(
      "recordings_prune",
      undefined,
      { deleted: 0 },
    );
    return r && typeof r === "object" ? (r.deleted ?? 0) : 0;
  },
  // recording_update_note(id, note) — map the renderer's timestamp key back to the
  // Rust row id (same map deleteHistoryEntry uses).
  updateHistoryNote: async (ts: number, note: string) => {
    const id = historyIdByTs.get(ts);
    if (!id) return false;
    return call("recording_update_note", { id, note: note || null }, false).then(() => true);
  },

  // ── Disk / recording ────────────────────────────────────────────────────
  // get_disk_space returns { freeBytes } (camelCase) — exactly what home.ts reads.
  getDiskSpace: async () =>
    call("get_disk_space", undefined, { freeBytes: null, totalBytes: null }),
  // Recording: the old renderer builds a full (old-shape) RecordingOpts, but the
  // Rust recorder wants its own RecordingOpts. plan_recording_opts builds the
  // correct one from the backend settings; we only forward customName/maxMinutes/
  // video from the old opts. (Device/format come from the Rust DB settings, not
  // the client-side localStorage settings — a known limit of the split.)
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
  stopRecordingNow: async () => call("stop_recording", undefined, true).then(() => true),
  // ── Auto-stop, owned by the recorder ───────────────────────────────────
  // The overlay's "+30 min" / "Avbryt auto-stopp" used to be renderer-local
  // setTimeouts that RE-implemented (and disagreed with) the engine's real
  // deadline. These three commands are the truth: extend/cancel move the
  // engine's watch value, the running loop re-pins its timer and re-emits
  // `recording://state` with the new `scheduled_stop_ms`, and the getter lets a
  // remounting overlay rehydrate the countdown without waiting for a transition.
  extendAutostop: async (minutes: number) =>
    invoke<void>("recording_extend_autostop", { minutes }),
  cancelAutostop: async () => invoke<void>("recording_cancel_autostop"),
  scheduledStopMs: async () =>
    call<number | null>("recording_scheduled_stop_ms", undefined, null),
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
    call<import("../bindings/PrerollStatus").PrerollStatus>("preroll_status", undefined, {
      active: false,
    }),
  // run_test_recording returns { ok, signal, sizeBytes, error }. The fallback
  // must match that shape ({ ok: false }) — the old { level, message } fallback
  // didn't match what the consumer reads.
  runTestRecording: async () =>
    call("run_test_recording", undefined, { ok: false }),
  // Precision capture bench: real recording argv for N s → ffprobed +
  // verdict-judged SelfTestReport (camelCase). Throws on hard failure so the
  // button can show the actual error text.
  // Real input channel count via the ffmpeg backend (getUserMedia caps at 2).
  probeDeviceChannels: async (deviceName: string) =>
    invoke<number>("probe_device_channels", { deviceName }),
  // Engine-side VU metering: starts the cpal stream on the device (negotiated
  // FULL channel count — a Qu-5's 32, not getUserMedia's 2) and streams
  // `vu-levels` events (~30/s, one peak+RMS entry per native channel) until
  // stopVu. Returns the negotiated channel count — the channel grid's width.
  startVu: async (deviceName: string | null) =>
    invoke<number>("start_vu", { deviceName }),
  stopVu: async () => invoke<void>("stop_vu"),
  // cpal device list (instant, no ffmpeg spawn): real max channel counts +
  // supported standard rates per input device.
  listInputDevices: async () =>
    invoke<import("../bindings/AudioDeviceList").AudioDeviceList>("list_input_devices"),
  // Per-channel peak scan — "which mixer channels carry the mix?"
  scanDeviceChannels: async (deviceName: string, secs: number) =>
    invoke<{ channel: number; peakDb: number }[]>("scan_device_channels", { deviceName, secs }),
  runCaptureBench: async (secs: number) =>
    invoke<import("../bindings/SelfTestReport").SelfTestReport>(
      "run_capture_bench",
      { secs },
    ),
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
  pickAudioFile: async () =>
    pickPath({ name: "Lyd", extensions: AUDIO_EXT }),

  // ── Email / webhook ─────────────────────────────────────────────────────
  //
  // These were `async () => ({ ok: false })` stubs: every click produced a
  // fabricated "sending failed" no matter what the user had configured. Both
  // commands exist and are registered (commands/email.rs), so they are wired —
  // and the panel now asks `emailStatus` FIRST and disables the button when
  // there is no send path, instead of inventing a failure.
  //
  // `email_test_webhook` is real on every build (plain reqwest POST, no cargo
  // feature); `email_send_test` needs `--features email` and returns a clear
  // `feature_disabled` error otherwise, which `emailStatus.featureBuilt`
  // predicts so we never provoke it.
  emailStatus: async () =>
    call<{ featureBuilt: boolean; gmailConnected: boolean }>("email_status", undefined, {
      featureBuilt: false,
      gmailConnected: false,
    }),
  testWebhook: async (url: string) => {
    try {
      const ok = await invoke<boolean>("email_test_webhook", { url });
      return ok ? { ok: true } : { ok: false, error: "unreachable" };
    } catch (e) {
      return { ok: false, error: ipcErrText(e) };
    }
  },
  testEmail: async (params: {
    transport: "gmail" | "smtp";
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
        transport: params.transport === "gmail" ? "Gmail" : "Smtp",
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
  clearSmtpPassword: async () =>
    call<boolean>("email_clear_smtp_password", undefined, false),

  // ── App / updates ───────────────────────────────────────────────────────
  getAppVersion: async () =>
    (await call<{ version?: string }>("app_info", undefined, {})).version ?? "—",
  getPlatform: async () => platform,
  // The menubar tray renders its labels in Rust, from a language it cannot read:
  // the UI language lives in THIS renderer's settings blob and was never part of
  // the curated `settings_save` payload. i18n.ts pushes it here on every locale
  // load. Best-effort — a build without the `tray` feature answers with a no-op.
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
      emitLocal("update-error", ("message" in st ? st.message : null) ?? `unexpected update phase: ${st.phase}`);
      return false;
    } catch (e) {
      if (timer !== undefined) clearInterval(timer);
      emitLocal("update-error", String(e));
      return false;
    }
  },
  // The purpose-built audio probe behind the Lyd tab's "Diagnose" button: one
  // enumeration, shaped into the flat name lists the panel renders. (The generic
  // `run_diagnostics` below is the whole-system report, still used for the
  // copy-to-support markdown.)
  diagnoseAudio: async () =>
    call<import("../bindings/AudioDiagnostics").AudioDiagnostics>(
      "diagnose_audio",
      undefined,
      { dshow: [], wasapi: [], wasapiAvailable: false },
    ),
  // Comprehensive diagnose: backend gathers system/devices/ffmpeg/disk/
  // permissions/audio-engine/last-error and returns structured `findings` (the
  // SR-* error codes) + a full markdown report. On failure → an empty report so
  // the panel still opens.
  runDiagnostics: async () =>
    call<{
      markdown: string;
      findings: {
        code: string;
        severity: "ok" | "info" | "warning" | "critical";
        title: string;
        detail: string;
        hint: string;
      }[];
      savedTo: string | null;
      captureOk: boolean | null;
      videoOk: boolean | null;
    }>("run_diagnostics", undefined, {
      markdown: "",
      findings: [],
      savedTo: null,
      captureOk: null,
      videoOk: null,
    }),

  // ── Health probes ───────────────────────────────────────────────────────
  // Two commands that existed since the port and were never called from
  // anywhere. `media_permissions` is the one that matters: a denied microphone
  // makes getUserMedia fail generically and avfoundation emit "Input/output
  // error", so the user was told the device was MISSING when macOS was simply
  // refusing it. AVFoundation knew all along.
  mediaPermissions: async () =>
    call<import("../bindings/MediaPermissions").MediaPermissions>(
      "media_permissions",
      undefined,
      { camera: "unknown", microphone: "unknown" },
    ),
  ffmpegHealth: async () =>
    call<import("../bindings/FfmpegHealth").FfmpegHealth>("ffmpeg_health", undefined, {
      available: true, // unknown ⇒ don't manufacture an alarm
      version: null,
      path: "",
    }),
  // Whether the OS login item is REALLY registered. The System tab used to show
  // the stored boolean, which drifts the moment a user removes the login item
  // by hand (or a migration/reinstall drops it) — a checkbox claiming scheduled
  // recordings survive a reboot when they don't.
  getLaunchAtLogin: async () => call<boolean>("get_launch_at_login", undefined, false),
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
  // ASIO driver names = the asio-backend entries of the unified tagged device
  // list (empty on macOS / when the `asio` feature is off / no driver installed,
  // so the picker simply shows no ASIO cards). See `audio::asio`.
  listAsioDrivers: async () => {
    const devs = await call<{ name: string; backend: string }[]>(
      "list_audio_devices",
      undefined,
      [],
    );
    return devs.filter((d) => d.backend === "asio").map((d) => d.name);
  },
  // The ASIO device's real input channels WITH driver labels, so the channel
  // grid can show channel names, not just numbers. Empty when ASIO is
  // unavailable → the caller falls back to a sensible default.
  listAsioInputChannels: async (deviceId: string) =>
    call<{ index: number; label: string }[]>(
      "list_audio_input_channels",
      { deviceId },
      [],
    ),
  // The ffmpeg/dshow audio inputs, for the "selected device not seen by ffmpeg"
  // warning. audio-page.ts calls `.some(...)` on the result → must be an array.
  listFfmpegAudioDevices: async () => {
    const inv = await call<{ audio_inputs?: { name: string; index: number }[] }>(
      "list_devices",
      undefined,
      {},
    );
    return (inv.audio_inputs ?? []).map((d) => ({ name: d.name, index: d.index }));
  },
  // list_devices → { video_inputs: FfmpegDevice[] }; old renderer wants
  // { name, index }[]. FfmpegDevice already carries both fields.
  listVideoDevices: async () => {
    const inv = await call<{ video_inputs?: { name: string; index: number }[] }>(
      "list_devices",
      undefined,
      {},
    );
    return (inv.video_inputs ?? []).map((d) => ({ name: d.name, index: d.index }));
  },
  // The SETUP-phase camera preview on HOME is client-side getUserMedia
  // (home.ts) — no backend involvement there. But the Direkte (live) page's
  // IDLE preview (live-page.ts startIdleCameraPreview/stopIdleCameraPreview)
  // calls THIS method, and it was a no-op stub that always reported success
  // while the `video-preview-frame` listener sat waiting for frames that never
  // arrived — a silently-dead preview (the backend `start_preview`/
  // `stop_preview` commands are real and already emit `preview://frame`,
  // mapped + base64-decoded above). `device` prefers the stored ffmpeg device
  // INDEX (a digit string resolves without enumeration, matching the
  // recorder's own device-token resolution); falls back to the device NAME for
  // a fuzzy match, or `null` for the default camera.
  videoPreviewStart: async (opts: unknown) => {
    const o = (opts ?? {}) as {
      videoDeviceName?: string | null;
      videoDeviceIndex?: number | null;
      videoFramerate?: number | null;
    };
    const device =
      o.videoDeviceIndex != null ? String(o.videoDeviceIndex) : o.videoDeviceName || null;
    try {
      await invoke("start_preview", { device, fps: o.videoFramerate ?? null });
      return true;
    } catch (e) {
      console.warn("[api-shim] start_preview failed", e);
      return false;
    }
  },
  videoPreviewStop: async () => {
    try {
      await invoke("stop_preview");
      return true;
    } catch (e) {
      console.warn("[api-shim] stop_preview failed", e);
      return false;
    }
  },
  // DURING recording the backend owns the camera and writes a preview JPEG to a
  // file; the renderer polls this (~base64 JPEG, or null when no fresh frame).
  recordingPreviewFrame: async () =>
    call<string | null>("recording_preview_frame", undefined, null),
  // Probe what the selected camera can actually capture, to gate the
  // resolution/fps UI. `token` is the device index (avfoundation) or name.
  // Returns null on failure → caller offers everything.
  getCameraCapabilities: async (token: string) =>
    call("get_camera_capabilities", { deviceToken: token }, null),

  // ── Wake from sleep (wake_* commands) ───────────────────────────────────
  // wake_reschedule returns WakeResult { ok, … }. A FAILED reschedule must report
  // ok:false — the old { ok:true } fallback painted a silent failure as success.
  scheduleOsWakes: async () =>
    call("wake_reschedule", undefined, { ok: false, reason: "error" }),
  scheduleOsWakesAdmin: async () =>
    call("wake_reschedule", undefined, { ok: false, reason: "error" }),
  // SleepConfig (wake_get_sleep_config) carries NO `platform` field — but the
  // schedule-page diagnostic branches on cfg.platform === 'darwin'/'win32' to pick
  // the right warnings, so without it every machine fell through to "unsupported
  // platform" (telling a Mac/Windows user wake won't work when it can). Inject the
  // platform the webview already knows; a real backend field (if ever added) wins.
  getSleepConfig: async () => ({
    platform,
    ...(await call<Record<string, unknown>>("wake_get_sleep_config", undefined, {})),
  }),
  fixMacSleep: async () => call("wake_fix_sleep", undefined, { ok: false }),
  fixWinWakeTimers: async () => call("wake_fix_sleep", undefined, { ok: false }),
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
  wakeVerifyScheduled: async () =>
    call("wake_verify", undefined, {
      expectedWakes: [],
      observedWakes: [],
      hasMismatch: false,
      onBattery: null,
      standbyEnabled: null,
    }),
  wakeTest: async (secondsAhead?: number) =>
    call("wake_test", { secondsAhead: secondsAhead ?? null }, { ok: false }),
  wakeCancelTest: async () => call("wake_cancel_test", undefined, true).then(() => true),
  wakeFailureHistory: async () => call("wake_failure_history", undefined, []),
  wakeClearFailureHistory: async () =>
    call("wake_clear_failure_history", undefined, true).then(() => true),

  // ── Editor ──────────────────────────────────────────────────────────────
  // Local path → asset:// URL for <audio>/<video> playback (WKWebView blocks
  // file://). Sync — convertFileSrc returns a string.
  toAssetUrl: (path: string) => toAssetUrl(path),
  // editor_read_file → { tooLarge, size, bytes }. The old loader expects EITHER
  // a raw byte array (→ Web Audio decode, the client-side waveform path) OR a
  // { tooLarge } marker (→ ffmpeg-extract fallback). Adapt to that.
  editorReadFile: async (fp: string) => {
    const r = await call<{ tooLarge?: boolean; bytes?: number[] | null }>(
      "editor_read_file",
      { mediaPath: fp },
      null as unknown as { tooLarge?: boolean; bytes?: number[] | null },
    );
    if (!r) return null;
    if (r.tooLarge) return { tooLarge: true };
    return new Uint8Array(r.bytes ?? []);
  },
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
    // Topic chapters (+title/speaker/description) ride along so they get
    // embedded as ID3 CHAP/CTOC. Chapters are { time, title } in seconds —
    // exactly EditorChapter; pass through, dropping any malformed entry.
    const chapters = Array.isArray(m.chapters)
      ? (m.chapters as Array<Record<string, unknown>>)
          .filter((c) => c && typeof c.time === "number" && typeof c.title === "string")
          .map((c) => ({ time: c.time as number, title: c.title as string }))
      : [];
    return editorCall(
      "editor_export",
      {
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
          chapters,
          title: (m.title as string) || null,
          speaker: (m.speaker as string) || null,
          description: (m.description as string) || null,
          vocalChainPreset: (o.vocalChainPreset as string) || null,
          processing: (o.processing as Record<string, unknown>) ?? null,
          channelRepair: (o.channelRepair as Record<string, unknown>) ?? null,
        },
      },
    );
  },
  // Analyse stereo channel balance → { code, imbalanceDb, peakLeftDb,
  // peakRightDb, recommended }. Throws-free: empty diagnosis on failure.
  editorDiagnoseChannels: async (fp: string) =>
    call("editor_diagnose_channels", { inputPath: fp }, null),
  // One-click "best result": diagnose + recommended preset bundle.
  editorAutoProcess: async (fp: string) =>
    call("editor_auto_process", { inputPath: fp }, null),
  // Kill the in-flight export's ffmpeg. Returns whether one was running; the
  // export itself then rejects with `cancelled`, which the editor maps to a
  // calm "Eksport avbrutt." (This was a stub returning `true` — the Avbryt
  // button did nothing and a 90-minute render was unkillable.)
  editorCancelExport: async () => call("editor_cancel_export", undefined, false),
  editorPickOutputFolder: async () => pickPath({ directory: true }),
  // Sidecars (meta / cutsDraft / transcript) are clean JSON key-value via
  // editor_read/write/delete_sidecar — no media decode needed.
  editorReadMeta: async (fp: string) =>
    call("editor_read_sidecar", { mediaPath: fp, sidecar: "meta" }, null),
  editorSaveMeta: async (fp: string, meta: unknown) =>
    call("editor_write_sidecar", { mediaPath: fp, sidecar: "meta", value: meta }, false).then(
      () => true,
    ),
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
    call("editor_delete_sidecar", { mediaPath: fp, sidecar: "cutsDraft" }, false).then(
      () => true,
    ),
  // editor_segments → EditorSegment[]. The consumer (editor/detection.ts) casts
  // the result directly to Suggestion[] and assigns E.suggestions, so return the
  // ARRAY, not a { segments } wrapper (which would make E.suggestions an object).
  // The result is cached in a `<stem>.segments.json` sidecar; `force` (the
  // explicit «Analyser opptak» button) re-runs the analysis instead of reading it.
  editorDetectSegments: async (fp: string, force?: boolean) =>
    call("editor_segments", { inputPath: fp, force: force ?? false }, []),
  // Topic chapters from the transcript (Bible refs + enumeration points). Pure
  // offline detection in Rust; returns [{ time, title }] on the original
  // recording timeline. Empty array on any failure (no transcript = no chapters).
  editorDetectChapters: async (lines: unknown, lang?: string) =>
    call("editor_detect_chapters", { lines: lines ?? [], lang: lang ?? null }, []),

  // R8 AI sermon companion — chapters + highlights + Norwegian summary from a
  // finished transcript. Deterministic detectors run on-device; the summary
  // uses the OPTIONAL Anthropic seam when a key is configured, else a local
  // extractive fallback (summarySource tells which). Returns null on any failure
  // so the panel shows a calm "ikke tilgjengelig" state rather than throwing.
  companionBuild: async (transcript: unknown, useLlm?: boolean) =>
    call(
      "companion_build",
      { transcript, useLlm: useLlm ?? null },
      null,
    ),
  // Whether the OPTIONAL LLM summary is wired (keychain or ANTHROPIC_API_KEY).
  companionLlmConfigured: async () =>
    call("companion_llm_configured", undefined, false),
  // Save/clear the Anthropic key in the OS keychain (never settings/bundle).
  companionSetLlmKey: async (key: string) =>
    call("companion_set_llm_key", { key }, false).then(() => true),
  companionClearLlmKey: async () =>
    call("companion_clear_llm_key", undefined, false).then(() => true),
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
    call<string | null>("editor_extract_playback_proxy", { inputPath: fp }, null),
  // editor_probe_peak → true max_volume (dBFS) of the ORIGINAL file via
  // volumedetect. Normalize's honest basis: the waveform peaks are an 8 kHz mono
  // downmix that under-reads the real peak by several dB.
  editorProbePeak: async (fp: string) =>
    call<number | null>("editor_probe_peak", { inputPath: fp }, null),
  editorPickVideoFile: async () =>
    pickPath({ name: "Video", extensions: VIDEO_EXT }),
  // Video export → editor_export with a video container (mp4/mov/mkv) + codec
  // (h264/h265). Maps the renderer params to EditorExportRequest just like
  // editorExportFile (the old raw-passthrough shape didn't match the request).
  editorExportVideo: async (params: unknown) => {
    const o = (params ?? {}) as Record<string, unknown>;
    const m = (o.metadata ?? {}) as Record<string, unknown>;
    const fmt = (o.videoFormat as string) || "mp4";
    const chapters = Array.isArray(m.chapters)
      ? (m.chapters as Array<Record<string, unknown>>)
          .filter((c) => c && typeof c.time === "number" && typeof c.title === "string")
          .map((c) => ({ time: c.time as number, title: c.title as string }))
      : [];
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
        chapters,
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
  // editor_probe_streams → EditorStreamInfo { hasVideo, hasAudio } | (on failure)
  // null. The old { streams: [] } fallback was the wrong shape: the consumer does
  // `!streams || streams.hasVideo`, so a truthy {streams:[]} made it read
  // .hasVideo (undefined) instead of taking the null branch. Return null.
  editorProbeStreams: async (fp: string) =>
    call("editor_probe_streams", { inputPath: fp }, null),
  editorReadTranscript: async (fp: string) =>
    call("editor_read_sidecar", { mediaPath: fp, sidecar: "transcript" }, null),
  editorWriteTranscript: async (fp: string, t: unknown) =>
    call(
      "editor_write_sidecar",
      { mediaPath: fp, sidecar: "transcript", value: t },
      false,
    ).then(() => true),
  editorDeleteTranscript: async (fp: string) =>
    call("editor_delete_sidecar", { mediaPath: fp, sidecar: "transcript" }, false).then(
      () => true,
    ),

  // ── Mastering (editor_master_* / editor_mastering_analyze) ──────────────
  // The 4 built-in mastering presets from the core (id/label/description +
  // targets/filters). Without this the preset dropdown was empty → the whole
  // mastering panel was unusable.
  masterPresets: async () => call("editor_master_presets", undefined, []),
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
  // The consumer reads `measureRes.ok` + `measureRes.measurement.inputI`, but the
  // Rust returns a FLAT EditorLoudness — wrap it under `measurement`.
  masterMeasure: async (inputPath: string, presetId: string) => {
    const r = await call<Record<string, unknown> | { ok: false }>(
      "editor_mastering_analyze",
      { inputPath, presetId },
      { ok: false },
    );
    if (r && typeof r === "object" && (r as { ok?: unknown }).ok === false) {
      return { ok: false };
    }
    return { ok: true, measurement: r };
  },
  masterApply: async (params: unknown) =>
    editorCall("editor_master_apply", { request: params }),
  masterCancel: async (jobId: string) =>
    call("editor_master_cancel", { jobId }, true).then(() => true),

  // ── Thumbnail ───────────────────────────────────────────────────────────
  thumbnailSetDefault: async () => ({ ok: false }),
  thumbnailClearDefault: async () => true,
  thumbnailSetEpisode: async () => ({ ok: false }),
  thumbnailClearEpisode: async () => true,
  thumbnailResolve: async () => null,
  thumbnailGetDefaultInfo: async () => null,

  // ── Cloud ───────────────────────────────────────────────────────────────
  cloudConnect: async () => okFalse,
  cloudCancelConnect: async () => true,
  cloudDisconnect: async () => true,
  cloudStatus: async () => cloudStatusStub,
  cloudUploadFile: async () => ({ ok: false }),
  cloudListFolders: async () => [],
  cloudSetFolder: async () => true,
  // Wired to the REAL predicate (commands/cloud.rs) instead of a hard-coded
  // `false`. It answers whether this build has a Google OAuth client id at all,
  // which is what the cloud panel's gate needs to say something true.
  cloudIsConfigured: async () => call<boolean>("cloud_is_configured", undefined, false),
  cloudQueueStatus: async () => ({ entries: [] }),
  cloudQueueRetry: async () => true,
  cloudQueueRemove: async () => true,
  cloudQueueFlush: async () => true,
  podcastRegenerate: async () => ({ ok: false }),
  registerTrustedPath: async () => true,

  // ── Gmail / YouTube ─────────────────────────────────────────────────────
  gmailConnect: async () => okFalse,
  gmailDisconnect: async () => true,
  gmailStatus: async () => ({ connected: false }),
  youtubeConnect: async () => okFalse,
  youtubeDisconnect: async () => true,
  youtubeStatus: async () => ({ connected: false }),
  youtubeUpload: async () => ({ ok: false }),

  // ── Streaming / overlays ────────────────────────────────────────────────
  // streamStatus shape (idle) matches old fields; live telemetry arrives via the
  // streaming://stats event. The action commands are wired:
  streamStatus: async () => call("stream_status", undefined, streamStatusStub),
  // stream_start resolves device tokens / snapshot / record-path itself from
  // settings — the renderer only sends the stream CONFIG. Map the resolution to
  // the backend enum's lowercase tag ("720p" → "p720"), pass full destination
  // views (incl. hasKey) + overlays, and surface the real error.
  streamStart: async (params: unknown) => {
    const p = (params ?? {}) as {
      resolution?: string;
      framerate?: number;
      videoBitrateKbps?: number;
      audioBitrateKbps?: number;
      destinations?: unknown[];
      overlays?: unknown[];
      alsoRecord?: boolean;
    };
    const resMap: Record<string, string> = {
      "480p": "p480",
      "720p": "p720",
      "1080p": "p1080",
    };
    try {
      const status = await invoke("stream_start", {
        destinations: p.destinations ?? [],
        resolution: resMap[p.resolution ?? "720p"] ?? "p720",
        framerate: p.framerate ?? 30,
        videoBitrateKbps: p.videoBitrateKbps ?? null,
        audioBitrateKbps: p.audioBitrateKbps ?? null,
        alsoRecord: !!p.alsoRecord,
        overlays: p.overlays ?? [],
      });
      return { ok: true, ...(status as object) };
    } catch (e) {
      return { ok: false, error: ipcErrText(e) };
    }
  },
  streamStop: async () => call("stream_stop", undefined, true).then(() => true),
  streamPreviewPath: async () => call("stream_preview_path", undefined, ""),
  streamSetKey: async (destId: string, key: string) =>
    call("stream_set_key", { destId, key }, true).then(() => true),
  streamDeleteKey: async (destId: string) =>
    call("stream_delete_key", { destId }, true).then(() => true),
  overlayListScreens: async () => [],
  overlayListNdiSources: async () => ({ available: false, sources: [] }),
  overlayPickImage: async () =>
    pickPath({ name: "Bilde", extensions: IMAGE_EXT }),

  // ── Transcripts / whisper ───────────────────────────────────────────────
  // The whole «Søk i prekener» full-text index (search-page.ts) is fed by this
  // ONE call — while it returned `[]` the sermon search silently found nothing
  // and the "N transkripsjoner indeksert" status stayed blank. `transcripts_list`
  // (commands/db.rs) walks the history, reads each `<name>.transcript.json`
  // sidecar and returns `{ basePath, transcript }` — `basePath` is the recording
  // path with its media extension stripped, which is exactly the join key
  // `baseNoExt(row.path)` the history rows use. Fallback `[]` keeps a missing
  // sidecar dir from breaking the page.
  transcriptListAll: async () =>
    call<Array<{ basePath: string; transcript: unknown }>>(
      "transcripts_list",
      undefined,
      [],
    ),
  // Render a transcript to SRT/VTT/TXT at a user-chosen path. Pure formatting +
  // one fs write in the backend (works in every build — no `whisper` feature).
  whisperExportTranscript: async (
    data: unknown,
    format: "srt" | "vtt" | "txt",
    path: string,
  ) => {
    try {
      await invoke("whisper_export_transcript", { data, format, path });
      return { ok: true as const };
    } catch (e) {
      return { ok: false as const, error: ipcErrText(e) };
    }
  },
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
  // whisper_list_models gives the catalogue; whisper_model_status the per-model
  // on-disk {installed, sizeOk}. The renderer's model picker needs both merged
  // (the old Electron whisper-status did this server-side) — without the
  // installed flags every transcription re-downloaded the model from scratch.
  whisperStatus: async () => {
    const models = await call<Array<Record<string, unknown>>>(
      "whisper_list_models",
      undefined,
      [],
    );
    const merged = await Promise.all(
      models.map(async (m) => ({
        ...m,
        ...(await call(
          "whisper_model_status",
          { id: m.id },
          { installed: false, sizeOk: false },
        )),
        id: m.id,
      })),
    );
    return {
      models: merged,
      installed: merged.filter((m) => m.installed).map((m) => m.id),
      active: null,
      binaryAvailable: true,
      available: true,
    };
  },
  // whisper_* commands take `id`, not `model_id`. The command returns `()` on
  // success and an AppError on failure; surface a real {ok,error} shape (the
  // generic `call` fallback would hide the reason → "feilet: undefined").
  whisperDownloadModel: async (modelId: string) => {
    try {
      await invoke("whisper_download_model", { id: modelId });
      return { ok: true as const };
    } catch (e) {
      const msg = ipcErrText(e);
      // The renderer suppresses the alert only for the exact "cancelled".
      return {
        ok: false as const,
        error: msg.endsWith("cancelled") ? "cancelled" : msg,
      };
    }
  },
  whisperCancelDownload: async (modelId: string) =>
    call("whisper_cancel_download", { id: modelId }, true).then(() => true),
  whisperDeleteModel: async (modelId: string) =>
    call("whisper_delete_model", { id: modelId }, true).then(() => true),
  // old { filePath, modelId, language, translate, jobId } → whisper_transcribe
  // (input_path, model_id, language, translate, subtitle_style, job_id). The
  // command returns the TranscriptData itself on success — wrap it in the
  // {ok, transcript} envelope the legacy renderer pattern-matches on, and map a
  // rejected invoke to {ok:false, error} (the renderer suppresses the alert for
  // the exact string "cancelled", so strip thiserror's "validation: " prefix).
  whisperTranscribe: async (params: unknown) => {
    const o = (params ?? {}) as Record<string, unknown>;
    try {
      const transcript = await invoke("whisper_transcribe", {
        inputPath: o.filePath,
        modelId: o.modelId,
        language: o.language ?? null,
        translate: o.translate ?? null,
        subtitleStyle: null,
        jobId: o.jobId ?? null,
      });
      return { ok: true as const, transcript };
    } catch (e) {
      const msg = ipcErrText(e);
      return {
        ok: false as const,
        error: msg.endsWith("cancelled") ? "cancelled" : msg,
      };
    }
  },
  whisperCancelTranscribe: async (jobId: string) =>
    call("whisper_cancel_transcribe", { jobId }, false),

  // ── Review queue ────────────────────────────────────────────────────────
  // `review_queue_list` (commands/review.rs) returns the persisted queue
  // newest-first with `ageInDays` filled in — already the renderer's
  // `ReviewQueueEntry` shape (camelCase), so no adaptation is needed. An empty
  // queue is the normal case: the home card hides itself on `[]`.
  reviewQueueList: async () =>
    call<ReviewQueueEntryLike[]>("review_queue_list", undefined, []),
  // No `review_queue_get` command exists — the queue is a single JSON blob, so
  // reading one entry means reading the list and picking. Cheap (a handful of
  // entries) and keeps the backend surface as it is.
  reviewQueueGet: async (id: string) => {
    const all = await call<ReviewQueueEntryLike[]>(
      "review_queue_list",
      undefined,
      [],
    );
    return all.find((e) => e?.id === id) ?? null;
  },
  // `review_mark_published` returns a bool: false = no such id in the queue
  // (already published, or the queue was cleared). Surface that as a real
  // reason instead of a silent no-op — the editor shows `error` in a dialog.
  reviewQueuePublish: async (id: string) => {
    try {
      const ok = await invoke<boolean>("review_mark_published", { id });
      return ok
        ? { ok: true as const }
        : { ok: false as const, error: "review_entry_not_found" };
    } catch (e) {
      return { ok: false as const, error: ipcErrText(e) };
    }
  },
  reviewQueueDiscard: async (id: string) =>
    call<boolean>("review_mark_discarded", { id }, false),
  reviewQueueUpdateTrim: async () => true,
  reviewQueueUpdateMasterPreset: async () => true,
  reviewQueueUpdateJingles: async () => true,

  // ── Integrations (Sunday-suite) ─────────────────────────────────────────
  getIntegrationSettings: async () => ({ enabled: false }),
  setIntegrationSettings: async () => ({ enabled: false }),
  getServiceLink: async () => null,
  sundayEditSend: async () => ({ ok: false }),
  sundayEditImport: async () => ({ ok: false }),
  stageImport: async () => ({ ok: false }),
  songSetApiKey: async () => true,
  songHasApiKey: async () => false,
  songSubmitUsage: async () => ({ ok: false }),
  planFetchServices: async () => [],
  planUpdateService: async () => ({ ok: false }),

  // ── Fire-and-forget (Electron ipcRenderer.send) ─────────────────────────
  notifyWeakSignal: noop,

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
    void listen(evt, (e) => fn(adapt ? adapt(e.payload) : e.payload)).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  },
};

(window as any).api = api;

// Seed the backend (sqlite) recording settings from localStorage ON BOOT, so a
// fresh launch where the user records without re-saving still uses their saved
// resolution/format/camera choices (not backend defaults). Best-effort.
void syncBackendRecordingSettings(loadSettings());
// Keep the OS login item in sync with the saved launch-at-login flag on boot
// (re-registers if the OS dropped it; idempotent otherwise).
void syncLaunchAtLogin(loadSettings());

// ── Native drag-drop bridge ───────────────────────────────────────────────
// Tauri intercepts OS file drags (dragDropEnabled defaults to true), so the
// legacy pages' HTML5 dragover/drop handlers never fire — and even if they
// did, Electron's non-standard `File.path` doesn't exist here. Bridge the
// native stream back into the DOM: re-dispatch synthetic DragEvents at the
// drop position with File objects carrying a real `path` property, so the
// editor's load/intro/outro zones and the thumbnail drop work unmodified.
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
  const [gotoPage, rawTab] = VERIFY_GOTO.split(":");
  // `settings:audio` and `settings:settings-audio` mean the same thing.
  const gotoTab = rawTab
    ? rawTab.startsWith(`${gotoPage}-`)
      ? rawTab
      : `${gotoPage}-${rawTab}`
    : undefined;
  const tryGoto = (): void => {
    const w = window as any;
    if (typeof w.showPage !== "function") {
      setTimeout(tryGoto, 50);
      return;
    }
    // No highlight pulse: this path exists to produce clean screenshots, and a
    // 4.4 s glow on the card would be in half of them.
    if (gotoTab) navigateTo(gotoPage, { tab: gotoTab, highlight: false });
    else w.showPage(gotoPage);
  };
  setTimeout(tryGoto, 150);
}
