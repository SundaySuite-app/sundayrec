/**
 * Seksjonene på Innstillinger-flaten, i rekkefølgen de står — og id-ene
 * innholdslista øverst hopper til.
 *
 * `id` er elementets `id`. `key` er suffikset i `app.setup.sections.*` (selve
 * overskriften). Rute-ankeret (`route.anchor`) er det samme navnet uten
 * `sec-`-prefikset — `SettingsPage` slår opp begge, så en lenke kan peke på en
 * seksjon (`sound`) eller på en rad inni den (`engine`, `autodelete`).
 */

export const SETTINGS_SECTIONS = [
  { id: "sec-sound", key: "sound" },
  { id: "sec-camera", key: "camera" },
  { id: "sec-folder", key: "folder" },
  { id: "sec-notify", key: "notify" },
  { id: "sec-language", key: "language" },
  { id: "sec-recording", key: "recording" },
  { id: "sec-updates", key: "updates" },
  { id: "sec-privacy", key: "privacy" },
  { id: "sec-about", key: "about" },
] as const;

export type SettingsSectionId = (typeof SETTINGS_SECTIONS)[number]["id"];
