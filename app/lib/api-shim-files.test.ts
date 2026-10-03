/**
 * `openFolder`/`revealFile` — hva shimmen sender, og hva den gjør med et nei.
 *
 * ## Hvorfor
 *
 * Webviewet har ingen `opener:`-tillatelse lenger. De to knappene som viser
 * noe i Finder/Utforsker går gjennom hver sin Rust-kommando
 * (`src-tauri/src/commands/recordings_open.rs`), og bakenden avgjør hva som
 * kan vises. Det denne fila pinner er SØMMEN mot den:
 *
 *   • `openFolder()` sender INGEN sti — bakenden finner opptaksmappa selv. Før
 *     ble kallet hoppet over når ingen mappe var valgt, som er standarden.
 *   • `revealFile(p)` sender stien ORDRETT: bakendens første sjekk er et
 *     eksakt treff mot historikkraden stien kom fra.
 *   • Et nei blir `false` — og havner i IPC-ringen diagnosepanelet leser.
 *     `openFolder` toaster selv (menylinja har ingen flate å si det på) — og
 *     en mappe som ikke finnes ennå, med egne ord, ikke «svarte ikke»;
 *     `revealFile` gjør det ikke (`app/ui/reveal.ts` sier sin egen setning, og
 *     to toaster for ett klikk er én for mye).
 *   • Ingenting i `app/` eller `legacy/` når opener-pluginen direkte.
 *
 * Verdenen bygges som i `api-shim-listen.test.ts`; `@tauri-apps/api/core`
 * byttes ut så hvert `invoke` kan leses av og besvares.
 */

import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import {
  afterAll,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";

const ipc = vi.hoisted(() => ({
  calls: [] as Array<{ cmd: string; args: unknown }>,
  answers: new Map<string, () => Promise<unknown>>(),
  inTauri: false,
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => {
    ipc.calls.push({ cmd, args });
    const answer = ipc.answers.get(cmd);
    return answer ? answer() : Promise.reject(new Error(`not mocked: ${cmd}`));
  },
  convertFileSrc: (p: string) => p,
  isTauri: () => ipc.inTauri,
}));

type Api = {
  openFolder: () => Promise<boolean>;
  revealFile: (p: string) => Promise<boolean>;
  getRecentIpcFailures: () => Array<{ cmd: string; error: string }>;
};

let api: Api;
const toasts: Array<{ kind: string; msg: string }> = [];

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
/** Fasiten for toastene: shimmen oversetter med den ekte katalogen. */
const NO = JSON.parse(
  readFileSync(join(repoRoot, "legacy/locales/no.json"), "utf8"),
) as { error: { recordingsFolderMissing: string; ipcFailed: string } };

beforeAll(async () => {
  const store = new Map<string, string>();
  const win: Record<string, unknown> = {
    localStorage: {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
      removeItem: (k: string) => void store.delete(k),
    },
    addEventListener: () => {},
    removeEventListener: () => {},
    matchMedia: () => ({ matches: false, addEventListener: () => {} }),
  };
  vi.stubGlobal("window", win);
  vi.stubGlobal("localStorage", win.localStorage);
  vi.stubGlobal("navigator", { userAgent: "node" });
  vi.stubGlobal("location", { search: "", href: "http://localhost/" });
  vi.stubGlobal("document", {
    createElement: () => ({
      style: {},
      classList: { add() {}, remove() {} },
      appendChild() {},
    }),
    body: { appendChild() {} },
    addEventListener: () => {},
    removeEventListener: () => {},
    getElementById: () => null,
    querySelector: () => null,
    querySelectorAll: () => [],
    documentElement: { lang: "no", setAttribute() {} },
  });
  vi.spyOn(console, "warn").mockImplementation(() => {});
  vi.spyOn(console, "error").mockImplementation(() => {});

  const shim = await import("./api-shim");
  shim.setShimNotifier({
    toast: (kind: string, msg: string) => void toasts.push({ kind, msg }),
  });
  api = (window as unknown as { api: Api }).api;
  // Fra nå av: som inne i appen, der en feilet `call` toaster.
  ipc.inTauri = true;
});

afterAll(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

beforeEach(() => {
  ipc.calls.length = 0;
  ipc.answers.clear();
  toasts.length = 0;
});

const callsTo = (cmd: string) => ipc.calls.filter((c) => c.cmd === cmd);

describe("openFolder", () => {
  it("ber bakenden om opptaksmappa uten å sende noen sti", async () => {
    ipc.answers.set("recordings_open_folder", async () => null);
    expect(await api.openFolder()).toBe(true);
    expect(callsTo("recordings_open_folder")).toEqual([
      { cmd: "recordings_open_folder", args: undefined },
    ]);
    expect(toasts).toEqual([]);
  });

  it("en mappe som ikke finnes ennå, sies med vanlige ord — og huskes", async () => {
    // Det vanlige førstegangstilfellet (opptakeren lager mappa ved første
    // opptak), eller en ekstern disk som ikke står i. Kommandoen SVARTE, med
    // en grunn — «Noe i bakgrunnen svarte ikke» ville vært feil å si.
    ipc.answers.set("recordings_open_folder", () =>
      Promise.reject({
        code: "validation",
        message:
          "validation: recordings_folder_missing: the recordings folder does not exist yet",
      }),
    );
    expect(await api.openFolder()).toBe(false);
    expect(toasts).toHaveLength(1);
    expect(toasts[0]?.kind).toBe("error");
    expect(toasts[0]?.msg).toBe(NO.error.recordingsFolderMissing);
    expect(api.getRecentIpcFailures()[0]).toMatchObject({
      cmd: "recordings_open_folder",
      error: expect.stringContaining("recordings_folder_missing"),
    });
  });

  it("et annet nei får den generelle setningen med kommandonavnet", async () => {
    // Forbi duplikatvinduet: én toast per kommando per minutt.
    const now = vi.spyOn(Date, "now").mockReturnValue(Date.now() + 10 * 60_000);
    try {
      ipc.answers.set("recordings_open_folder", () =>
        Promise.reject({
          code: "validation",
          message:
            "validation: recordings_folder_is_a_package: the recordings folder is an application or package and is not opened",
        }),
      );
      expect(await api.openFolder()).toBe(false);
      expect(toasts).toHaveLength(1);
      expect(toasts[0]?.msg).toContain(NO.error.ipcFailed);
      expect(toasts[0]?.msg).toContain("(recordings_open_folder)");
    } finally {
      now.mockRestore();
    }
  });

  it("koden toasten bygger på, sendes fortsatt av Rust", () => {
    // Skjøten: en omdøpt kode i Rust ville gjort toasten over generell igjen,
    // stille.
    const rust = readFileSync(
      join(repoRoot, "src-tauri/src/commands/recordings_open.rs"),
      "utf8",
    );
    expect(rust).toContain('"recordings_folder_missing: ');
  });
});

describe("revealFile", () => {
  it("sender stien ordrett til recordings_reveal", async () => {
    ipc.answers.set("recordings_reveal", async () => null);
    const path = "/Users/kantor/Documents/SundayRec/Søndag 4. okt 11.00.mp3";
    expect(await api.revealFile(path)).toBe(true);
    expect(callsTo("recordings_reveal")).toEqual([
      { cmd: "recordings_reveal", args: { path } },
    ]);
  });

  it("svarer false og husker feilen, men toaster ikke — reveal.ts sier fra selv", async () => {
    ipc.answers.set("recordings_reveal", () =>
      Promise.reject({
        code: "validation",
        message:
          "validation: reveal_not_allowed: only recordings and exports from this session can be shown",
      }),
    );
    expect(await api.revealFile("/etc/hosts")).toBe(false);
    expect(toasts).toEqual([]);
    expect(api.getRecentIpcFailures()[0]).toMatchObject({
      cmd: "recordings_reveal",
      error: expect.stringContaining("reveal_not_allowed"),
    });
  });
});

describe("ingen vei til opener-pluginen fra webviewet", () => {
  const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

  function sources(dir: string): string[] {
    return readdirSync(dir).flatMap((name) => {
      const full = join(dir, name);
      if (name === "node_modules" || name === "bindings") return [];
      if (statSync(full).isDirectory()) return sources(full);
      return /\.(ts|tsx|js|mjs)$/.test(name) && !/\.test\.tsx?$/.test(name)
        ? [full]
        : [];
    });
  }

  it("app/ og legacy/ importerer ikke @tauri-apps/plugin-opener og kaller ingen plugin:opener-kommando", () => {
    const files = [
      ...sources(join(repo, "app")),
      ...sources(join(repo, "legacy")),
    ];
    // Leser den i det hele tatt? En skanner som fant null filer er alltid grønn.
    expect(files.length).toBeGreaterThan(50);
    const offenders = files.filter((f) => {
      const src = readFileSync(f, "utf8");
      return (
        src.includes("@tauri-apps/plugin-opener") ||
        src.includes("plugin:opener")
      );
    });
    expect(offenders.map((f) => relative(repo, f))).toEqual([]);
  });

  it("package.json har ikke lenger opener-pluginen som avhengighet", () => {
    const pkg = JSON.parse(readFileSync(join(repo, "package.json"), "utf8"));
    expect(pkg.dependencies?.["@tauri-apps/plugin-opener"]).toBeUndefined();
  });
});
