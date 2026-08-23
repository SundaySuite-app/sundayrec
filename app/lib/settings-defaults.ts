// The renderer-side copy of `Settings::default()` (R4).
//
// TWO consumers, both outside the normal data path:
//
//   1. api-shim's `getSettings` FALLBACK — used only when `settings_get`
//      itself fails (broken backend/db). The UI must still render something,
//      and that something is the same defaults the backend would have
//      answered with. The failure is surfaced loudly (toast) — this object is
//      never a silent substitute for the store.
//   2. `e2e/harness.ts` — the base a spec's partial `settings` seed is merged
//      over, standing in for Rust's merge-over-defaults.
//
// It is deliberately typed as the GENERATED binding, so a field added to (or
// removed from) the Rust struct fails compilation here instead of drifting:
// the field SET cannot go stale. The VALUES mirror `Settings::default()` in
// `crates/sundayrec-core/src/settings.rs` — if you change a default there,
// change it here in the same commit (the Rust `defaults_match_electron` test
// and this file are the two spellings of that one truth).
//
// NOT a source of truth for the app: the store is sqlite, read via
// `settings_get`, which answers with real (validated) values.

import type { Settings } from "../../legacy/bindings/Settings";

export const SETTINGS_DEFAULTS: Settings = {
  // System
  language: null,
  onboardingDone: false,

  // Audio device
  deviceId: null,
  deviceName: null,
  deviceChannels: {},

  // Video device
  videoEnabled: false,
  videoDeviceName: null,
  videoDeviceIndex: null,
  videoFlip: false,
  keepSeparateAudio: true,
  classicDirectshow: false,
  classicFfmpegAudio: false,
  classicFfmpegPreroll: false,

  // Audio processing
  channels: "stereo",
  inputChannelL: null,
  inputChannelR: null,
  sampleRateMode: "auto",

  // Output
  format: "mp3",
  bitrate: "256",
  filenamePattern: "date",
  saveFolder: null,
  autoDeleteDays: 0,

  // Recording behaviour
  stopOnSilence: false,
  silenceThreshold: -50,
  silenceTimeoutMinutes: 5,
  splitMinutes: 0,
  manualMaxMinutes: 0,
  // 15, not 0 (P1b): «pre-roll on and invisible» is the owner's choice, and
  // this file is the renderer-side spelling of `Settings::default()`.
  preRollSeconds: 15,
  prerollEnabled: false,
  reminderMinutes: 0,

  // System behaviour
  launchAtLogin: false,
  wakeFromSleep: true,
  protectRecording: true,

  // Schedule
  // `true` mirrors the Rust `default = "default_true"`: the flag separates
  // «armed» from «configured», and an older profile without the key keeps
  // planning its stored slots.
  autoRecordEnabled: true,
  slots: [],
  specialRecordings: [],

  // Church profile
  churchName: "",
  responsiblePerson: "",

  // Notifications
  notifyStart: true,
  notifyStop: true,

  // Email alerts
  emailOnError: false,
  emailAddress: "",
  emailSmtp: "",
  emailSmtpPort: 587,
  emailSmtpUser: "",
  emailSmtpFrom: "",

  // Editor
  editorIntroPath: null,
  editorOutroPath: null,

  // Misc
  autoUpdate: true,
  updateChannel: "stable",
  askOpenEditor: true,
};
