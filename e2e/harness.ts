import type { Page } from "@playwright/test";
import { SETTINGS_DEFAULTS } from "../app/lib/settings-defaults";

// The one way a spec boots the app.
//
// Everything a spec needs in place BEFORE the renderer's module scripts run
// goes in one `addInitScript`: `window.__SUNDAYREC_FIXTURES__` — the E5.1
// seam. Outside Tauri fixtures are honoured unconditionally (there is no
// backend to shadow), so no query param is needed here; see
// app/lib/fixtures-core.ts.
//
// SETTINGS are fixtures like everything else since R4 (`settings_get` /
// `settings_save` invokes — the localStorage store is dead). The harness
// installs a tiny FIXTURE-BACKED STORE standing in for the backend's sqlite
// row: a spec's partial `settings` seed is merged over `SETTINGS_DEFAULTS`
// (emulating Rust's merge-over-defaults), every `settings_save` replaces the
// stored object and is recorded for `settingsSavePayloads`, and the store
// itself survives `page.reload()` — persistence across a teardown is exactly
// what the reload specs assert. A spec can still override any of the three
// commands with its own fixture; the store handlers are defaults.

/** Where the harness's fake settings row lives between reloads. Not an app
 *  key — the app never reads it — just the emulated backend's disk. */
export const SETTINGS_DB_KEY = "__e2e.settingsDb";

/** A canned answer per Tauri command name: a value, or a function of the args. */
export type Fixtures = Record<string, unknown>;

export interface BootOptions {
  /** Canned command answers. */
  fixtures?: Fixtures;
  /** Seeded settings, merged over the shim's defaults by `loadSettings`. */
  settings?: Record<string, unknown>;
  /**
   * `?goto=<page>[:<tab>]`, in the OLD shell's vocabulary — `home`, `schedule`,
   * `settings`, `search` (that is the library; there never was a `history`
   * page), `editor`, and the `settings:<tab>` forms. Those ids are still the
   * ones written here because they are still the ones out in the world (the
   * tray, deep links, this suite); `PAGE_ALIASES` / `TAB_ALIASES` in
   * `app/router/router.ts` translate every one of them to a destination in the
   * new three-place navigation, and `app/router/router.test.ts` has a row per
   * form that appears in this repo.
   *
   * ⚠️ `?goto=` also forces `onboardingDone` true so a deep-linked boot skips
   * first-run. Any spec that wants the first-run sequence must boot WITHOUT it
   * (see onboarding.spec.ts, first-run.spec.ts).
   */
  goto?: string;
}

/**
 * Serialise fixtures across the `addInitScript` boundary.
 *
 * Functions do not survive structured clone, so anything that needs to branch on
 * the invoke args is passed as SOURCE and rebuilt in the page. A spec writes
 * `fn("(a) => a.sidecar === 'meta' ? {...} : null")`; everything else is plain
 * data and goes over untouched.
 */
const FN_MARKER = "__sundayrec_fn__";
const VOID_MARKER = "__sundayrec_void__";

/** Wrap a function SOURCE string so `boot` rebuilds it inside the page. */
export function fn(source: string): unknown {
  return { [FN_MARKER]: source };
}

/**
 * A fixture that answers with `undefined` — the many void commands
 * (`settings_save`, `stop_vu`, `editor_allow_asset_path`, …).
 *
 * Needed because a bare `undefined` does not survive the `addInitScript`
 * argument boundary: Playwright's serialiser DROPS undefined-valued object
 * properties, so `{ editor_allow_asset_path: undefined }` arrives as `{}` and
 * the command falls through to a live invoke that then throws. That failure is
 * silent and looks like a hung screen, so it gets an explicit sentinel.
 */
export const VOID: unknown = { [VOID_MARKER]: true };

/**
 * Boot the renderer with fixtures + settings installed, and wait until it has
 * actually painted (`window.showPage` exists — the same signal `?goto=` polls
 * for), so a spec never races the first frame.
 */
let bootSeq = 0;

export async function boot(page: Page, opts: BootOptions = {}): Promise<void> {
  const payload = {
    fixtures: opts.fixtures ?? {},
    settings: opts.settings ?? null,
    defaults: SETTINGS_DEFAULTS as unknown as Record<string, unknown>,
    dbKey: SETTINGS_DB_KEY,
    marker: FN_MARKER,
    voidMarker: VOID_MARKER,
    // An init script runs on EVERY navigation in the context, including
    // `page.reload()`. Re-seeding there would silently overwrite whatever the
    // app just persisted — i.e. it would make every "does this survive a
    // reload?" assertion a test of the seed. So each `boot()` seeds exactly
    // once, tracked by its own sentinel; a reload then keeps what the app wrote.
    seedOnce: `__e2e_seeded_${++bootSeq}`,
  };

  await page.addInitScript((p) => {
    const w = window as unknown as Record<string, unknown>;

    // ── The non-IPC half of the Tauri runtime ────────────────────────────────
    //
    // `call()` guards every `invoke`, which is why the renderer survives in a
    // browser at all. But `convertFileSrc`, `listen` and `getCurrentWebview` go
    // to `window.__TAURI_INTERNALS__` DIRECTLY, and outside Tauri that object
    // does not exist — so they throw `TypeError` rather than degrade. In the
    // editor that is fatal: `toAssetUrl` throws inside `loadAudioFile`, the load
    // promise rejects, and the screen sits on «Analyserer…» forever.
    //
    // So the harness supplies the runtime, not the backend. Note what is
    // deliberately NOT set: `window.isTauri`. `isTauri()` reads only that, so it
    // stays FALSE — fixtures keep being honoured unconditionally, and E2.4's
    // failure toast stays suppressed, exactly as in a real browser boot.
    // `internals.invoke` rejects, because "no backend" is the truth here; a
    // honoured fixture short-circuits before ever reaching it.
    if (!w.__TAURI_INTERNALS__) {
      let nextCallbackId = 1;
      const callbacks = new Map<number, (payload: unknown) => void>();
      w.__TAURI_INTERNALS__ = {
        transformCallback(cb: (payload: unknown) => void) {
          const id = nextCallbackId++;
          callbacks.set(id, cb);
          (w as Record<string, unknown>)[`_${id}`] = cb;
          return id;
        },
        unregisterCallback(id: number) {
          callbacks.delete(id);
        },
        invoke(cmd: string) {
          // Event subscription is plumbing, not a backend call: `listen()` goes
          // through `invoke` too, and rejecting it turns every `window.api.on…`
          // in the renderer into an unhandled rejection in the console. Resolve
          // it with an id — the subscription simply never fires, which is the
          // truth (nothing is emitting) and is quiet about it.
          if (cmd === "plugin:event|listen")
            return Promise.resolve(nextCallbackId++);
          if (cmd === "plugin:event|unlisten")
            return Promise.resolve(undefined);
          return Promise.reject(
            new Error(`no Tauri backend in the browser tier: ${cmd}`),
          );
        },
        // Real Tauri hands back an `asset://`/`http://asset.localhost` URL. Media
        // will not play either way (there is no file), so this only has to be a
        // string the DOM accepts in a `src`.
        convertFileSrc(filePath: string, protocol = "asset") {
          return `${protocol}://localhost/${encodeURIComponent(filePath)}`;
        },
        metadata: {
          currentWindow: { label: "main" },
          currentWebview: { label: "main" },
        },
      };
    }

    // `@tauri-apps/api`'s `unlisten` goes to a SECOND internals object
    // (`__TAURI_EVENT_PLUGIN_INTERNALS__.unregisterListener`) that only the
    // real event plugin installs. Without it every page-switch that tears a
    // listener down (e.g. home → settings stopping the VU feed) threw an
    // unhandled TypeError into the console. A no-op is the truth here: the
    // harness's `plugin:event|listen` never registers anything to remove.
    if (!w.__TAURI_EVENT_PLUGIN_INTERNALS__) {
      w.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener() {},
      };
    }

    const revive = (v: unknown): unknown => {
      if (v && typeof v === "object") {
        const o = v as Record<string, unknown>;
        if (p.voidMarker in o) return undefined;
        if (p.marker in o) {
          return new Function(`return (${o[p.marker] as string})`)();
        }
      }
      return v;
    };
    const map: Record<string, unknown> = {};
    for (const [cmd, value] of Object.entries(p.fixtures))
      map[cmd] = revive(value);

    // ── The fixture-backed settings store (stands in for sqlite) ────────────
    // Installed as DEFAULT handlers — a spec's own fixture for any of these
    // commands wins. The store is a localStorage row under a harness-only key,
    // which is what lets a value survive `page.reload()` the way sqlite would.
    const db = {
      read: (): Record<string, unknown> => {
        const raw = window.localStorage.getItem(p.dbKey);
        return raw
          ? (JSON.parse(raw) as Record<string, unknown>)
          : { ...p.defaults };
      },
      write: (v: Record<string, unknown>): void =>
        window.localStorage.setItem(p.dbKey, JSON.stringify(v)),
    };
    if (!("settings_get" in map)) map.settings_get = () => db.read();
    if (!("settings_save" in map))
      map.settings_save = (args?: Record<string, unknown>) => {
        const s = (args?.settings ?? {}) as Record<string, unknown>;
        db.write(s);
        ((w.__settingsSaves ??= []) as unknown[]).push(s);
        return s;
      };
    if (!("settings_import" in map))
      map.settings_import = (args?: Record<string, unknown>) => {
        // Emulates the backend's merge-over-defaults (`Settings::from_json_merged`).
        const merged = {
          ...p.defaults,
          ...(JSON.parse((args?.json as string) ?? "{}") as Record<
            string,
            unknown
          >),
        };
        db.write(merged);
        ((w.__settingsImports ??= []) as unknown[]).push(args?.json);
        return merged;
      };

    // Fixtures ARE re-installed every navigation: they stand in for a backend,
    // and a backend does not disappear because the page reloaded.
    w.__SUNDAYREC_FIXTURES__ = map;

    if (window.localStorage.getItem(p.seedOnce)) return;
    window.localStorage.setItem(p.seedOnce, "1");
    // Seed the fake sqlite row exactly once per boot(): a reload then keeps
    // what the app saved, so "survives a reload" assertions test the app.
    window.localStorage.setItem(
      p.dbKey,
      JSON.stringify({ ...p.defaults, ...(p.settings ?? {}) }),
    );
  }, payload);

  await page.goto(opts.goto ? `/?goto=${encodeURIComponent(opts.goto)}` : "/");
  await page.waitForFunction(
    () =>
      typeof (window as unknown as Record<string, unknown>).showPage ===
      "function",
  );
}

/**
 * A settings object that suppresses first-run.
 *
 * The first-run gate reads `onboardingDone` alone, so this is what every spec
 * except the first-run ones wants. (`?goto=` sets it too, but being explicit
 * means a spec that later drops `?goto=` does not silently gain a wizard.)
 */
export const SETTLED_SETTINGS = { onboardingDone: true };

/**
 * The commands the app touches on EVERY boot, answered with something harmless.
 *
 * Without these each one rejects, lands in E2.4's failure ring and renders its
 * empty state — which is fine, but it means a spec asserting "the screen is
 * populated" is fighting noise from six unrelated screens. Spread this and
 * override the handful the journey is actually about.
 */
export const BOOT_FIXTURES: Fixtures = {
  app_info: { version: "0.10.0-e2e" },
  scheduler_status: { next: null },
  scheduler_reschedule: VOID,
  // The retention pass runs unasked on every boot; `disabled` keeps it silent
  // (no toast) so it cannot photobomb an unrelated spec's assertions.
  recordings_prune: { moved: 0, disabled: true },
  get_disk_space: { freeBytes: 250_000_000_000, totalBytes: 500_000_000_000 },
  recordings_list: [],
  trash_list: [],
  list_audio_devices: [],
  email_status: { featureBuilt: false },
  email_has_smtp_password: false,
  get_launch_at_login: false,
  // `needsPrompt: false` matters: a `true` here floats the one-time consent card
  // over every other screen and every other spec's assertions.
  telemetry_consent_get: {
    status: "denied",
    version: 2,
    decidedAt: 1_754_000_000_000,
    currentVersion: 2,
    needsPrompt: false,
    active: false,
  },
  telemetry_queue_status: {
    pending: 0,
    failed: 0,
    oldestAt: null,
    lastError: null,
  },
  telemetry_count: VOID,
};

/**
 * Every FULL settings object `settings_save` has received so far — recorded by
 * the harness's default store handler at the exact boundary where the object
 * leaves the renderer on its way to sqlite. The seam specs (settings-seam,
 * update-channel) pin the R4 invariant on this observable: the payload is the
 * whole vocabulary, so a field written is a field read back — nothing curated,
 * nothing silently re-defaulted (the #113 family's ending).
 */
export async function settingsSavePayloads(
  page: Page,
): Promise<Array<Record<string, unknown>>> {
  return page.evaluate(
    () =>
      (
        window as unknown as {
          __settingsSaves?: Array<Record<string, unknown>>;
        }
      ).__settingsSaves ?? [],
  );
}

/** The fake sqlite row as it stands right now — what a fresh `settings_get`
 *  would answer. The storage-layer observable for "it actually persisted". */
export async function storedSettings(
  page: Page,
): Promise<Record<string, unknown>> {
  return page.evaluate(
    (key) =>
      JSON.parse(window.localStorage.getItem(key) ?? "{}") as Record<
        string,
        unknown
      >,
    SETTINGS_DB_KEY,
  );
}

/** Every raw JSON string `settings_import` has received (the migration path). */
export async function settingsImportPayloads(page: Page): Promise<string[]> {
  return page.evaluate(
    () =>
      (window as unknown as { __settingsImports?: string[] })
        .__settingsImports ?? [],
  );
}

/** One `recordings_list` row. NOTE: this command answers in snake_case. */
export function recordingRow(
  over: Partial<Record<string, unknown>> = {},
): Record<string, unknown> {
  return {
    id: "rec-1",
    file_path: "/Users/test/Opptak/gudstjeneste.mp3",
    device_name: "Qu-5",
    started_at: 1_754_400_000_000,
    duration_ms: 3_600_000,
    byte_size: 86_000_000,
    created_at: 1_754_400_000_000,
    note: null,
    ...over,
  };
}
