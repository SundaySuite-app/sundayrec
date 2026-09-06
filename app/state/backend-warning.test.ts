/**
 * `backend://warning` — kanalen ingen hørte på.
 *
 * Fire ting bevises her, og den første er den viktigste: at det finnes en
 * lytter i det hele tatt. Resten er formen på det den gjør — kode → nøkkel,
 * dedupliseringen mot flatene skallet allerede har, og pre-roll-brikka som
 * slukker når bufferen er død.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { banners, clearBanners } from "./banners";
import {
  currentSurfaces,
  initBackendWarnings,
  interpolate,
  planWarning,
  warningParams,
  WARNING_BANNER_KEYS,
  WARNING_SUFFIXES,
  type ExistingSurfaces,
} from "./backend-warning";
import { audioDevices } from "./devices";
import { diskFreeBytes } from "./disk";
import { prerollActive } from "./preroll";
import { settings } from "./settings";
import { LOW_DISK_MINUTES } from "./status-line";
import { sourceState } from "../pages/record/record-core";
import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";

const NOTHING_UP: ExistingSurfaces = {
  lowDiskShown: false,
  deviceMissingShown: false,
};

afterEach(() => {
  clearBanners();
  settings.value = { ...SETTINGS_DEFAULTS };
  audioDevices.value = null;
  diskFreeBytes.value = null;
  prerollActive.value = false;
});

// ── Tabellen mot Rust ───────────────────────────────────────────────────────

describe("kodetabellen mot sundayrec_core::notify::code", () => {
  /** `code::ALL` slik den STÅR i kjernen — ikke en kopi av den. */
  function rustCodes(): string[] {
    const src = readFileSync(
      join(import.meta.dirname, "../../crates/sundayrec-core/src/notify.rs"),
      "utf8",
    );
    const mod = /pub mod code \{([\s\S]*?)\n\}/.exec(src);
    if (!mod) throw new Error("fant ikke `pub mod code` i notify.rs");
    const all = /pub const ALL: &\[&str\] = &\[([^\]]*)\]/.exec(mod[1]);
    if (!all) throw new Error("fant ikke `code::ALL` i notify.rs");
    const names = all[1]
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    return names.map((name) => {
      const decl = new RegExp(`pub const ${name}: &str = "([^"]+)"`).exec(
        mod[1],
      );
      if (!decl) throw new Error(`fant ingen verdi for ${name}`);
      return decl[1];
    });
  }

  // MUTASJONSPRØVEN: legg en femte kode i `code::ALL` i Rust (eller fjern en
  // rad her), og begge disse blir røde. Det er hele poenget — en ny advarsel
  // bakenden lærer skal ikke kunne bli en tom stripe i skallet uten at noe
  // sier fra i gaten.
  it("dekker nøyaktig kodene Rust erklærer", () => {
    expect(Object.keys(WARNING_BANNER_KEYS).sort()).toEqual(rustCodes().sort());
  });

  it("har en katalognøkkel per kode", () => {
    expect(Object.keys(WARNING_SUFFIXES).sort()).toEqual(rustCodes().sort());
  });

  it("har teksten i BEGGE de aktive katalogene, i alle sju", () => {
    // De sju er parity-testens ansvar; det denne legger til er at nøkkelen
    // faktisk er DEN nøkkelen `warningText` slår opp — en `notify.*` som bare
    // finnes i no.json ville gitt en tom stripe på engelsk.
    for (const lang of ["no", "en", "sv", "da", "de", "fr", "pl"]) {
      const cat = JSON.parse(
        readFileSync(
          join(import.meta.dirname, `../../legacy/locales/${lang}.json`),
          "utf8",
        ),
      ) as { notify?: Record<string, string> };
      for (const suffix of Object.values(WARNING_SUFFIXES)) {
        expect(
          typeof cat.notify?.[suffix] === "string" && cat.notify[suffix].length,
          `${lang}.json mangler notify.${suffix}`,
        ).toBeTruthy();
      }
    }
  });
});

// ── Planen ──────────────────────────────────────────────────────────────────

describe("planWarning", () => {
  it("gir hver kjent kode SIN egen bannernøkkel", () => {
    for (const [code, key] of Object.entries(WARNING_BANNER_KEYS)) {
      const plan = planWarning(
        { code, severity: "warn", params: {} },
        NOTHING_UP,
      );
      expect(plan.action, code).toBe("raise");
      if (plan.action !== "raise") return;
      expect(plan.banner.key, code).toBe(key);
    }
  });

  it("en UKJENT kode blir en stripe med motorens egen setning, ikke stillhet", () => {
    const plan = planWarning(
      { code: "fan_failure", msg: "Viften har stoppet.", severity: "error" },
      NOTHING_UP,
    );
    expect(plan.action).toBe("raise");
    if (plan.action !== "raise") return;
    expect(plan.banner.key).toBe("backend-warning");
    expect(plan.banner.msg).toBe("Viften har stoppet.");
    expect(plan.banner.severity).toBe("error");
  });

  it("hverken kode eller setning ⇒ ingenting å si", () => {
    expect(planWarning({ params: {} }, NOTHING_UP).action).toBe("ignore");
    expect(planWarning(null, NOTHING_UP).action).toBe("ignore");
    expect(planWarning("nei", NOTHING_UP).action).toBe("ignore");
  });

  it("ukjent alvorlighetsgrad leses som `warn`, ikke som rop", () => {
    const plan = planWarning(
      { code: "disk_low", severity: "katastrofe" },
      NOTHING_UP,
    );
    if (plan.action !== "raise") throw new Error("ventet raise");
    expect(plan.banner.severity).toBe("warn");
  });

  // ── Dedupliseringsregelen ────────────────────────────────────────────────
  //
  // MUTASJONSPRØVEN: fjern de to `deduped`-grenene i planWarning, og disse to
  // blir røde — som er nøyaktig «to bannere om det samme».
  it("`disk_low` tier når opptakssidens egen diskstripe allerede står", () => {
    expect(
      planWarning({ code: "disk_low" }, { ...NOTHING_UP, lowDiskShown: true })
        .action,
    ).toBe("deduped");
  });

  it("`device_missing` tier når «Finner ikke …» allerede står", () => {
    expect(
      planWarning(
        { code: "device_missing" },
        { ...NOTHING_UP, deviceMissingShown: true },
      ).action,
    ).toBe("deduped");
  });

  it("…men SIER det når den flaten ikke står — det er halvdelen som betyr noe", () => {
    // Scheduleren varsler en halvtime før et planlagt opptak, og da har ingen
    // åpnet opptakssiden. Å droppe advarselen der ville vært å gjeninnføre
    // feilen i det ene tilfellet den koster en gudstjeneste.
    expect(planWarning({ code: "device_missing" }, NOTHING_UP).action).toBe(
      "raise",
    );
    expect(planWarning({ code: "disk_low" }, NOTHING_UP).action).toBe("raise");
  });
});

describe("warningParams / interpolate", () => {
  it("regner om `freeBytes` til noe et menneske kan lese", () => {
    expect(warningParams({ params: { freeBytes: "3221225472" } }).freeGb).toBe(
      "3.0",
    );
  });

  it("lar en ukjent plassholder STÅ — en synlig {file} er en feilrapport", () => {
    expect(interpolate("mistet {file} og {x}", { file: "a.flac" })).toBe(
      "mistet a.flac og {x}",
    );
  });
});

// ── Flatene skallet allerede har ────────────────────────────────────────────

describe("currentSurfaces", () => {
  it("er enig med opptakssidens egen `sourceState` om «enheten er borte»", () => {
    // ⚠️ Den ene testen som ikke handler om denne fila: dedupliseringen hviler
    // på at de to reglene sier det samme, og de er skrevet hver for seg
    // (`soundChosen` her, `sourceState` der). Blir de uenige, får brukeren
    // enten to stripper om samme sak eller ingen.
    const cases = [
      { deviceId: "x32", deviceName: "Qu-5", devices: [{ id: "builtin" }] },
      { deviceId: "x32", deviceName: "Qu-5", devices: [{ id: "x32" }] },
      { deviceId: null, deviceName: null, devices: [{ id: "x32" }] },
      { deviceId: "x32", deviceName: "Qu-5", devices: null },
    ];
    for (const c of cases) {
      settings.value = {
        ...SETTINGS_DEFAULTS,
        deviceId: c.deviceId,
        deviceName: c.deviceName,
      };
      audioDevices.value = c.devices as never;
      expect(currentSurfaces().deviceMissingShown, JSON.stringify(c)).toBe(
        sourceState(settings.value, c.devices as never).kind ===
          "source-missing",
      );
    }
  });

  it("leser diskstripa av de samme minuttene opptakssiden gjør", () => {
    settings.value = { ...SETTINGS_DEFAULTS };
    // Rikelig plass ⇒ ingen stripe; nesten ingenting ⇒ stripe.
    diskFreeBytes.value = 500_000_000_000;
    expect(currentSurfaces().lowDiskShown).toBe(false);
    diskFreeBytes.value = 1_000_000;
    expect(currentSurfaces().lowDiskShown).toBe(true);
    expect(LOW_DISK_MINUTES).toBeGreaterThan(0);
  });
});

// ── Lytteren, som var det som manglet ───────────────────────────────────────

describe("initBackendWarnings", () => {
  /** Et minimalt `window.api` med bare `on`, og en vei til å fyre kanalen. */
  function withFakeApi(): {
    emit: (payload: unknown) => void;
    off: () => void;
    channels: string[];
  } {
    const handlers: Array<(p: unknown) => void> = [];
    const channels: string[] = [];
    (globalThis as unknown as { window: unknown }).window = {
      api: {
        on(channel: string, fn: (p: unknown) => void) {
          channels.push(channel);
          if (channel === "backend-warning") handlers.push(fn);
          return () => {};
        },
      },
    };
    const dispose = initBackendWarnings();
    return {
      emit: (payload) => handlers.forEach((h) => h(payload)),
      off: () => {
        dispose();
        delete (globalThis as unknown as { window?: unknown }).window;
      },
      channels,
    };
  }

  // MUTASJONSPRØVEN, og hele funnet: fjern `window.api.on("backend-warning", …)`
  // fra initBackendWarnings, og alle tre under blir røde. Før denne runden
  // fantes abonnementet ikke i det hele tatt — bakenden ropte, og ingen var der.
  it("abonnerer faktisk på kanalen", () => {
    const h = withFakeApi();
    expect(h.channels).toContain("backend-warning");
    h.off();
  });

  it("reiser et banner per kode, nøklet så gjentakelser ikke stables", async () => {
    const h = withFakeApi();
    h.emit({
      code: "recovery_skipped",
      severity: "warn",
      params: { file: "a" },
    });
    h.emit({
      code: "recovery_skipped",
      severity: "warn",
      params: { file: "b" },
    });
    h.emit({ code: "preroll_dead", severity: "warn", params: {} });
    await Promise.resolve();
    const keys = banners.value.map((b) => b.key);
    expect(keys).toEqual(["backend-recovery-skipped", "backend-preroll-dead"]);
    h.off();
  });

  it("PREROLL_DEAD slukker «Lytter»-brikka, ikke bare stripa", async () => {
    // Brikka sto over en død buffer fordi `prerollActive` bare ble skrevet av
    // det siste `preroll_start` som svarte ja. Den løgnen er hele grunnen til
    // at pre-roll finnes: løftet om sekundene ingen kan ta om igjen.
    const h = withFakeApi();
    prerollActive.value = true;
    h.emit({ code: "preroll_dead", severity: "warn", params: {} });
    await Promise.resolve();
    expect(prerollActive.value).toBe(false);
    expect(banners.value.map((b) => b.key)).toContain("backend-preroll-dead");
    h.off();
  });
});
