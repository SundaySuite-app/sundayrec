import { test, expect, type Page } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  SETTINGS_SAVE_SPY,
  settingsSavePayloads,
} from "./harness";

// The renderer→sqlite settings seam, held to account for EVERY key with a
// backend reader — the #113 bug ("updateChannel never crossed") generalised.
//
// The seam: the UI's full settings live in localStorage; the backend reads
// sqlite via `settings::load(db)`, which only ever sees the CURATED object
// `backendRecordingSettings` sends through `settings_save`. Rust deserialises
// that object with `#[serde(default)]` on every field, so a key that is read
// backend-side but absent from the curated object is not merely "not synced" —
// it is actively RE-DEFAULTED on every save. Both halves stay green: the
// renderer really saves, the backend really defaults, and the seam between
// them silently drops the field. That shape produced #113 (updateChannel), and
// this spec pins the five further keys a reader-by-reader enumeration of
// `settings.*` uses behind `settings::load` found in the same hole:
//
//   autoDeleteDays  → commands/db.rs `recordings_prune`: retention was always
//                     0 = pruning permanently disabled, whatever the UI said.
//   showLiveLevels  → scheduler/mod.rs `live_levels`: the astats meter tap for
//                     scheduled recordings (no UI writer today; the sync keeps
//                     an imported/hand-set value from being re-defaulted).
//   inputVolume,
//   trimSilence,
//   launchAtLogin   → core telemetry's environment block + the diagnose
//                     report: every install claimed the defaults.
//
// Observable: the `settings_save` payload itself, spied at the invoke
// boundary — the exact place the curated subset leaves the renderer.

// The `settings_save` spy + payload reader live in harness.ts — shared with
// update-channel.spec.ts, which pins the same seam.
const FIXTURES = { ...BOOT_FIXTURES, settings_save: SETTINGS_SAVE_SPY };

/** The boot sync's curated payload (the first `settings_save` after boot). */
async function bootPayload(page: Page): Promise<Record<string, unknown>> {
  await expect
    .poll(() => settingsSavePayloads(page).then((p) => p.length))
    .toBeGreaterThan(0);
  const saves = await settingsSavePayloads(page);
  return saves[0];
}

test.describe("settings seam — every backend-read key crosses with its value", () => {
  test("non-default values reach the backend save, not just localStorage", async ({
    page,
  }) => {
    // Seed the localStorage blob the way a real install that USED these
    // settings would have it, then let the module-load sync run. Value
    // transport is the property: presence alone would pass a bridge that
    // hardcodes the defaults.
    await boot(page, {
      fixtures: FIXTURES,
      settings: {
        ...SETTLED_SETTINGS,
        autoDeleteDays: 90,
        showLiveLevels: false,
        inputVolume: 80,
        trimSilence: true,
        launchAtLogin: true,
      },
      goto: "settings:general",
    });

    const payload = await bootPayload(page);
    expect(payload.autoDeleteDays).toBe(90);
    expect(payload.showLiveLevels).toBe(false);
    expect(payload.inputVolume).toBe(80);
    expect(payload.trimSilence).toBe(true);
    expect(payload.launchAtLogin).toBe(true);
  });

  test("an untouched install sends the Rust defaults, and numbers stay integers", async ({
    page,
  }) => {
    // The defaults must MATCH Rust's per-field defaults (0 / true / 100 /
    // false / false): the sync fires on every boot, so a wrong default here is
    // not neutral — it would overwrite sqlite with a value the user never set.
    //
    // `inputVolume` is additionally seeded with a FLOAT: serde rejects a
    // fractional number for an `i32`, and one bad field fails the WHOLE
    // settings_save (dropping all recorder sync with it) — so the bridge must
    // integer-coerce before the invoke, the same trap `filenamePattern`
    // documents.
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...SETTLED_SETTINGS, inputVolume: 80.5 },
      goto: "settings:general",
    });

    const payload = await bootPayload(page);
    expect(payload.autoDeleteDays).toBe(0);
    expect(payload.showLiveLevels).toBe(true);
    expect(payload.trimSilence).toBe(false);
    expect(payload.launchAtLogin).toBe(false);
    expect(Number.isInteger(payload.inputVolume)).toBe(true);
    expect(payload.inputVolume).toBe(81);
  });
});
