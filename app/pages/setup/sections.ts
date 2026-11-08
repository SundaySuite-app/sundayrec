/**
 * Seksjonene på Innstillinger-flaten, i rekkefølgen de står — og id-ene
 * innholdslista øverst hopper til.
 *
 * `id` er elementets `id` (ikke et rute-anker: `route.anchor` peker på kortene
 * INNE i seksjonene, som `engine`, `schedule` og `publish`). `title` er
 * katalognøkkelen til overskriften.
 */

export const SETTINGS_SECTIONS = [
  { id: "sec-church", key: "church" },
  { id: "sec-schedule", key: "schedule" },
  { id: "sec-publish", key: "publish" },
  { id: "sec-recording", key: "recording" },
  { id: "sec-updates", key: "updates" },
  { id: "sec-help", key: "help" },
  { id: "sec-about", key: "about" },
] as const;

export type SettingsSectionId = (typeof SETTINGS_SECTIONS)[number]["id"];
