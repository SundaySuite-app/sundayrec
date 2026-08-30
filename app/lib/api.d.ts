// The `window` contract, and the only place it is written down.
//
// It lived in `legacy/renderer/main.ts` — the old shell's entry point — which
// fase B deleted along with the rest of that shell. The declaration is not the
// old shell's, though: it describes what `api-shim.ts` INSTALLS and what the
// shell on top of it installs back, so it belongs beside the shim.
//
// It is trimmed to exactly the surface that exists. 45 methods went with the
// pages that were their only callers (`masterApply`, `runDiagnostics`,
// `updateHistoryNote`, the `wake*` interactive set, the ASIO/ffmpeg device
// lists, …); every one of them is named in the fase-B PR and in
// docs/APP-SHELL.md under «Etter byttet», because a Rust command whose only
// door closed is a decision, not a leak. Declaring a method here that the shim
// does not install would be worse than not declaring it: the type would promise
// a function that is `undefined` at run time.
//
// `.d.ts`, not a module: this is ambient, and every file in the program sees it
// without importing anything.

import type { Settings, EditorSegment } from "../../legacy/types";
import type { TrashEntry } from "../../legacy/bindings/TrashEntry";
import type { WakeResult } from "../../legacy/bindings/WakeResult";
import type { WakeStatus } from "../../legacy/bindings/WakeStatus";

declare global {
  interface Window {
    /**
     * Navigate. The ONE global the new shell installs — the tray, the deep
     * links and `e2e/harness.ts` all rest on it, and the harness waits for it
     * as the signal that the shell has finished booting.
     *
     * ⚠️ `window.loadSettings`, `window.showOnboarding` and `window.__isRecording`
     * are deliberately NOT recreated by `app/` — each was a second place that
     * believed it knew a current value. `__isRecording` is still declared
     * because `app/lib/audio/vu-feed.ts` READS it: the read is a guard against
     * starting `start_vu` while a recording owns the device, and with nobody
     * writing the flag the guard is inert. That is safe here and only here,
     * because the shell guards the same thing by MOUNTING — no meter in the
     * tree, no `start_vu` — which is written down at the call site in
     * `app/pages/record/RecordPage.tsx`. Rebuilding the flag would be a second
     * writer on one truth, which is the failure class this shell exists to end.
     * Collapsing the two into an argument is the standing restanse; see «Etter
     * byttet» in `docs/APP-SHELL.md`.
     */
    showPage: (id: string) => void;
    /** Set to true while a recording owns the device. Nothing writes it — see above. */
    __isRecording?: boolean;
    /** Open the editor on a file (drag-and-drop, and the library row). */
    openEditorWithFile: (filePath: string, seekToSec?: number) => void;
    api: {
      getSettings: () => Promise<Settings>;
      saveSettings: (s: Settings) => Promise<boolean>;
      /** Write the whole (validated) settings object to a user-chosen JSON
       *  file. Rejects on failure — the profile card shows the reason. */
      settingsExportToFile: (path: string) => Promise<void>;
      /** Import a settings JSON file (merge-over-defaults + validate) and
       *  return the stored result. Rejects on failure. */
      settingsImportFromFile: (path: string) => Promise<Settings>;
      /** Open-dialog picker for a settings-profile JSON. Cancel → null. */
      pickSettingsFile: () => Promise<string | null>;
      getNextRecording: () => Promise<{ date: string } | null>;
      getHistory: () => Promise<unknown[]>;
      deleteHistoryEntry: (ts: number) => Promise<void>;
      /** Move recordings (with sidecars + video sibling) into the papirkurv.
       *  Rejects rather than reporting a delete that did not happen. */
      trashMove: (paths: string[]) => Promise<TrashEntry[]>;
      /** Everything currently recoverable, newest first. */
      trashList: () => Promise<TrashEntry[]>;
      /** Put one entry back where it came from. */
      trashRestore: (id: string) => Promise<TrashEntry>;
      /** Destroy entries permanently — an empty list empties the papirkurv. */
      trashPurge: (ids: string[]) => Promise<number>;
      getDiskSpace: () => Promise<{ freeBytes: number | null }>;
      startRecordingNow: (
        opts: unknown,
      ) => Promise<{ ok?: boolean; error?: string }>;
      stopRecordingNow: () => Promise<boolean>;
      /** Push the running recording's auto-stop deadline out by `minutes`.
       *  Adds to the live deadline, so it can never shorten it. Rejects on
       *  failure — the overlay says so rather than pretending. */
      recordingExtendAutostop: (minutes: number) => Promise<void>;
      /** Drop the auto-stop entirely: record until someone presses stop.
       *  Rejects on failure, same reason. */
      recordingCancelAutostop: () => Promise<void>;
      /** One base64 JPEG from the engine's preview sink, or `null` when it has
       *  not written a frame yet. Only meaningful DURING a recording — the
       *  recorder owns the camera then, so this is the only way to see it. */
      recordingPreviewFrame: () => Promise<string | null>;
      /** Start the rolling pre-roll buffer. Resolves false when the backend
       *  declined (pre-roll off in its settings copy, or no device matched). */
      prerollStart?: () => Promise<boolean>;
      /** Stop the rolling pre-roll buffer (safe when nothing is running). */
      prerollStop?: () => Promise<void>;
      /** Whether the rolling pre-roll buffer is actually running. */
      prerollStatus?: () => Promise<{ active: boolean }>;
      runPreflight: () => Promise<{
        findings: {
          severity: "warn" | "error";
          category: string;
          message: string;
        }[];
      }>;
      pickFolder: () => Promise<string | null>;
      /** Open a folder in the OS file manager. Resolves FALSE when the
       *  opener refused — the shim catches, so the boolean is the only place
       *  the difference survives (it answered `boolean` all along; the type
       *  said `void`). */
      openFolder: (p: string) => Promise<boolean>;
      /** Reveal a file in Finder/Explorer. Same contract as `openFolder`. */
      revealFile: (p: string) => Promise<boolean>;
      /** Store the SMTP password in the OS keychain (undefined/'' clears it).
       *  Resolves true when a password is now stored. Rejects on a keychain
       *  failure — the caller must show it, not swallow it. */
      emailSetSmtpPassword: (password?: string) => Promise<boolean>;
      /** Whether an SMTP password is stored. The secret never crosses back. */
      emailHasSmtpPassword: () => Promise<boolean>;
      /** Whether this build can send e-mail at all — read BEFORE offering a
       *  «Send test» (see feature-gate). */
      emailStatus: () => Promise<
        import("../../legacy/bindings/EmailStatus").EmailStatus
      >;
      testEmail: (params: {
        recipient: string;
        language?: string;
        host?: string;
        port?: number;
        user?: string;
        pass?: string;
        from?: string;
      }) => Promise<{ ok: boolean; error?: string }>;
      getAppVersion: () => Promise<string>;
      checkForUpdates: () => Promise<void>;
      installUpdate: () => void;
      /** Push the UI language to the Rust menubar tray (it renders its own labels). */
      traySetLanguage?: (code: string) => Promise<void>;
      /** macOS camera + microphone authorization (AVFoundation), for preflight. */
      mediaPermissions?: () => Promise<{
        camera: import("../../legacy/bindings/AuthStatus").AuthStatus;
        microphone: import("../../legacy/bindings/AuthStatus").AuthStatus;
      }>;
      /** Whether the bundled ffmpeg sidecar resolved, and where. */
      ffmpegHealth?: () => Promise<{
        available: boolean;
        version: string | null;
        path: string;
      }>;
      /** Whether the OS login item is really registered (not the stored boolean). */
      getLaunchAtLogin?: () => Promise<boolean>;
      /** Trackpad haptic tap (macOS Force Touch); a silent no-op elsewhere. */
      hapticPerform?: (pattern: string) => Promise<void>;
      wakeDetectCapabilities: () => Promise<{
        platform: "mac-arm" | "mac-intel" | "win" | "linux" | "other";
        canWakeFromSleep: boolean;
        canWakeFromOff: boolean;
        needsAdmin: boolean;
        knownIssues: string[];
        recommendations: string[];
      }>;
      /**
       * (Re)register the OS wake timers for the coming schedule, NOW. User-
       * initiated, so it is allowed to prompt for admin — which is why it is a
       * button in Avansert and not something the scheduler does silently.
       * `ok:false` + `reason` when it did not happen.
       */
      wakeReschedule: () => Promise<WakeResult>;
      /**
       * What the OS says it has actually scheduled. The hero's «Maskinen vekkes
       * automatisk kl. …» is rendered off THIS, not off the stored setting: the
       * setting is an intention, this is the fact.
       */
      wakeVerifyScheduled: () => Promise<WakeStatus>;
      on: (
        channel: string,
        fn: (...args: unknown[]) => void,
      ) => (() => void) | undefined;
      toAssetUrl: (path: string) => string;
      editorPickFile: () => Promise<string | null>;
      editorExportFile: (
        params: unknown,
      ) => Promise<{ ok: boolean; outputPath?: string; error?: string }>;
      /** Kill the in-flight export render; resolves to whether one was running. */
      editorCancelExport: () => Promise<boolean>;
      editorPickOutputFolder: () => Promise<string | null>;
      // The generated binding, not a hand-written twin — see `Suggestion` in
      // pages/editor/state.ts for what the twin cost us.
      editorDetectSegments: (
        filePath: string,
        force?: boolean,
      ) => Promise<EditorSegment[]>;
      /** Persist a sermon-pick correction (E8). Resolves to whether it was
       *  recorded — re-picking the detector's own block is not a correction. */
      editorRecordSermonPick: (
        filePath: string,
        request: import("../../legacy/bindings/EditorSermonPickRequest").EditorSermonPickRequest,
      ) => Promise<boolean>;
      /** Index into `segments` of the block the human corrected us to, or null. */
      editorSermonPick: (
        filePath: string,
        segments: EditorSegment[],
      ) => Promise<number | null>;
      editorAutoProcess: (filePath: string) => Promise<{
        diagnosis: {
          code: string;
          recommended: { mode: string; leftDb: number; rightDb: number };
        };
        vocalChainPreset: string;
        masterPreset: string;
        summary: string;
      } | null>;
      editorReadCutsDraft: (filePath: string) => Promise<unknown>;
      editorSaveCutsDraft: (filePath: string, cuts: unknown) => Promise<void>;
      editorDeleteCutsDraft: (filePath: string) => Promise<void>;
      /** The backend-tagged input list — the renderer's ONLY audio-device
       *  enumeration since the getUserMedia label blink-open was removed. */
      listAudioDevices: () => Promise<
        import("../../legacy/bindings/TaggedAudioInput").TaggedAudioInput[]
      >;
      startVu: (deviceName: string | null) => Promise<number>;
      stopVu: () => Promise<void>;
      registerTrustedPath: (filePath: string) => Promise<boolean>;
      /** Native "save as" picker — returns the chosen path, or null on cancel. */
      pickSavePath: (opts: {
        defaultPath?: string;
        name?: string;
        extensions?: string[];
      }) => Promise<string | null>;
      /** The cameras ffmpeg can see. REJECTS when the read failed — an empty
       *  list means "no cameras", and the two must not look alike. */
      listVideoDevices: () => Promise<{ name: string; index: number }[]>;
      getCameraCapabilities: (token: string) => Promise<{
        maxWidth: number;
        maxHeight: number;
        maxFps: number;
        supportedResolutions: string[];
        supportedFramerates: number[];
      } | null>;
      editorLoadRecording: (filePath: string) => Promise<{
        durationSec: number;
        hasVideo: boolean;
        hasAudio: boolean;
        channels: number | null;
        sampleFmt: string | null;
        sampleRate: number | null;
      } | null>;
      editorAllowAssetPath: (filePath: string) => Promise<boolean>;
      editorExtractAudioPeaks: (
        filePath: string,
      ) => Promise<{ peaks: number[]; sampleRate: number } | null>;
      editorExtractPlaybackProxy: (filePath: string) => Promise<string | null>;
      editorExportVideo: (
        params: unknown,
      ) => Promise<{ ok: boolean; outputPath?: string; error?: string }>;
      masterPreview: (
        inputPath: string,
        presetId: string,
        startSec: number,
        durationSec: number,
      ) => Promise<{ ok: boolean; previewPath?: string; error?: string }>;
      /** Reveal the rotating log folder in Finder/Explorer (falls back to the
       *  folder itself before the first line is written). No path in, none out —
       *  resolves to whether the OS actually opened something. */
      logsReveal: () => Promise<boolean>;
      /** The tail of the live log file, clamped server-side to 512 KB
       *  regardless of `maxBytes`. Empty string means nothing logged yet. */
      logsTail: (maxBytes: number) => Promise<string>;
      /** Every IPC failure remembered this session (renderer-local ring,
       *  newest first) — answers even when the backend that IS failing can't
       *  be asked. */
      getRecentIpcFailures: () => import("./ipc-failures-core").IpcFailure[];
      // ── Telemetry (E3.6) — opt-in, anonymous, off by default. The rest of
      // the surface (preview payload, queue status, delete-my-data) is
      // declared in E3.7's block below. ──────────────────────────────────
      /** The current consent state — status/never-asked-granted-denied, the
       *  derived needsPrompt/active — see crates/sundayrec-core/telemetry/
       *  consent.rs for the state machine this mirrors. */
      telemetryConsentGet: () => Promise<
        import("../../legacy/bindings/TelemetryConsent").TelemetryConsent
      >;
      /** Record the user's answer. Resolves `null` ONLY on a real IPC
       *  failure — callers must never treat `null` as "recorded", since the
       *  whole point of asking once is that a lost answer must be asked
       *  again. */
      telemetryConsentSet: (
        granted: boolean,
      ) => Promise<
        import("../../legacy/bindings/TelemetryConsent").TelemetryConsent | null
      >;
      // ── Telemetry (E3.7) — the settings-panel surface ────────────────────
      /** The real next payload as pretty JSON, honestly labelled — see
       *  TelemetryPreview's own doc comment for why the two consent states
       *  answer differently. `null` only on a genuine IPC failure; never
       *  fabricated. */
      telemetryPreviewPayload: () => Promise<
        import("../../legacy/bindings/TelemetryPreview").TelemetryPreview | null
      >;
      /** "Slett mine data", the local half: retires the install id. Resolves
       *  `false` only on a real failure. */
      telemetryRegenerateInstallId: () => Promise<boolean>;
    };
    appVersion?: string;
  }
}

export {};
