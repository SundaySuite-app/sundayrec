/**
 * `openFolder`, «Vis i Finder», papirkurven, sidevognene og mappevelgeren —
 * hva shimmen sender, og hva den gjør med et nei.
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
 *   • `revealRecording(id)`/`revealExport(token)` sender en historikkrads id
 *     eller eksportens lapp — ALDRI en sti (B-familien, PR-D).
 *   • Et nei blir `false` — og havner i IPC-ringen diagnosepanelet leser.
 *     `openFolder` toaster selv (menylinja har ingen flate å si det på) — og
 *     en mappe som ikke finnes ennå, med egne ord, ikke «svarte ikke»;
 *     «Vis i Finder» gjør det ikke (`app/ui/reveal.ts` sier sin egen setning,
 *     og to toaster for ett klikk er én for mye).
 *   • `trashMove(ids)`, sidevognene og prekenvalget navngir opptaket med id
 *     eller File-lapp, og shimmen har ikke lenger et eneste `mediaPath`.
 *   • `settingsPickSaveFolder()` sender ingenting: Rust åpner mappevelgeren.
 *   • Ingenting i `app/` eller `legacy/` når opener- eller dialog-pluginen
 *     direkte.
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
  revealRecording: (recordingId: string) => Promise<boolean>;
  revealExport: (exportToken: string) => Promise<boolean>;
  trashMove: (recordingIds: string[]) => Promise<unknown>;
  settingsPickSaveFolder: () => Promise<
    { ok: true; settings: unknown } | { ok: false; error: string }
  >;
  editorReadContent: (sourceToken: string) => Promise<unknown>;
  editorSaveContent: (
    sourceToken: string,
    content: unknown,
  ) => Promise<boolean>;
  editorDeleteContent: (sourceToken: string) => Promise<boolean>;
  editorReadCutsDraft: (sourceToken: string) => Promise<unknown>;
  editorSaveCutsDraft: (sourceToken: string, cuts: unknown) => Promise<boolean>;
  editorDeleteCutsDraft: (sourceToken: string) => Promise<boolean>;
  editorRecordSermonPick: (
    sourceToken: string,
    request: unknown,
  ) => Promise<boolean>;
  editorSermonPick: (
    sourceToken: string,
    segments: unknown,
  ) => Promise<unknown>;
  settingsExportProfile: () => Promise<boolean>;
  settingsImportProfile: () => Promise<unknown>;
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

const TOKEN = "11111111-1111-4111-8111-111111111111";

describe("«Vis i Finder» — en id eller en lapp, aldri en sti (B-familien)", () => {
  it("revealRecording sender radens id til recordings_reveal og ingenting annet", async () => {
    ipc.answers.set("recordings_reveal", async () => null);
    expect(await api.revealRecording("rad-7")).toBe(true);
    expect(callsTo("recordings_reveal")).toEqual([
      { cmd: "recordings_reveal", args: { recordingId: "rad-7" } },
    ]);
  });

  it("revealExport sender eksportens lapp til recordings_reveal_export", async () => {
    ipc.answers.set("recordings_reveal_export", async () => null);
    expect(await api.revealExport(TOKEN)).toBe(true);
    expect(callsTo("recordings_reveal_export")).toEqual([
      { cmd: "recordings_reveal_export", args: { exportToken: TOKEN } },
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
    expect(await api.revealRecording("finnes-ikke")).toBe(false);
    expect(toasts).toEqual([]);
    expect(api.getRecentIpcFailures()[0]).toMatchObject({
      cmd: "recordings_reveal",
      error: expect.stringContaining("reveal_not_allowed"),
    });
  });

  it("shimmen har ikke lenger en revealFile som tar en sti", () => {
    expect(
      (api as unknown as Record<string, unknown>).revealFile,
    ).toBeUndefined();
  });
});

describe("papirkurven — radenes id-er, ikke stiene (B1)", () => {
  it("trashMove sender recordingIds", async () => {
    ipc.answers.set("trash_move", async () => []);
    await api.trashMove(["rad-1", "rad-2"]);
    expect(callsTo("trash_move")).toEqual([
      { cmd: "trash_move", args: { recordingIds: ["rad-1", "rad-2"] } },
    ]);
  });

  it("en id Rust ikke kjenner når kallstedet, som den er", async () => {
    const refusal = {
      code: "validation",
      message: "validation: recording_unknown: no recording in the history",
    };
    ipc.answers.set("trash_move", () => Promise.reject(refusal));
    await expect(api.trashMove(["rad-x"])).rejects.toEqual(refusal);
  });
});

describe("sidevognene og prekenvalget — opptakets lapp, ikke stien (A3/A4)", () => {
  // MUTASJONSPRØVEN: send `mediaPath` i stedet for `sourceToken` i én av dem,
  // og den blir rød — Rust tar ikke lenger imot en sti her.
  beforeEach(() => {
    for (const cmd of [
      "editor_read_sidecar",
      "editor_write_sidecar",
      "editor_delete_sidecar",
      "editor_record_sermon_pick",
      "editor_sermon_pick",
    ]) {
      ipc.answers.set(cmd, async () => null);
    }
  });

  it("hver sidevogn-kommando sender sourceToken og sidevognens art", async () => {
    await api.editorReadContent(TOKEN);
    await api.editorSaveContent(TOKEN, {
      title: "T",
      speaker: "",
      description: "",
    });
    await api.editorDeleteContent(TOKEN);
    await api.editorReadCutsDraft(TOKEN);
    await api.editorSaveCutsDraft(TOKEN, [{ start: 1, end: 2 }]);
    await api.editorDeleteCutsDraft(TOKEN);
    const sent = ipc.calls.map((c) => ({
      cmd: c.cmd,
      args: c.args as Record<string, unknown>,
    }));
    expect(sent.map((c) => [c.cmd, c.args.sidecar])).toEqual([
      ["editor_read_sidecar", "meta"],
      ["editor_write_sidecar", "meta"],
      ["editor_delete_sidecar", "meta"],
      ["editor_read_sidecar", "cutsDraft"],
      ["editor_write_sidecar", "cutsDraft"],
      ["editor_delete_sidecar", "cutsDraft"],
    ]);
    for (const { args } of sent) {
      expect(args.sourceToken).toBe(TOKEN);
      expect(Object.keys(args)).not.toContain("mediaPath");
    }
  });

  it("prekenvalget sendes og leses med lappen", async () => {
    await api.editorRecordSermonPick(TOKEN, { chosenIndex: 2 });
    await api.editorSermonPick(TOKEN, []);
    expect(callsTo("editor_record_sermon_pick")).toEqual([
      {
        cmd: "editor_record_sermon_pick",
        args: { sourceToken: TOKEN, request: { chosenIndex: 2 } },
      },
    ]);
    expect(callsTo("editor_sermon_pick")).toEqual([
      {
        cmd: "editor_sermon_pick",
        args: { sourceToken: TOKEN, segments: [] },
      },
    ]);
  });

  it("shimmen sender ikke en sti til noen kommando — `mediaPath` er borte", () => {
    const shim = readFileSync(join(repoRoot, "app/lib/api-shim.ts"), "utf8");
    expect(shim).not.toContain("mediaPath");
    expect(shim).not.toContain('recordings_reveal", { path');
    expect(shim).not.toMatch(/invoke[^;]*\{\s*paths\b/);
  });
});

describe("opptaksmappen — Rust åpner mappevelgeren og lagrer den (A2-familien)", () => {
  it("settingsPickSaveFolder sender ingen argumenter", async () => {
    const stored = { saveFolder: "/Volumes/Kirke/Opptak" };
    ipc.answers.set("settings_pick_save_folder", async () => stored);
    expect(await api.settingsPickSaveFolder()).toEqual({
      ok: true,
      settings: stored,
    });
    expect(callsTo("settings_pick_save_folder")).toEqual([
      { cmd: "settings_pick_save_folder", args: undefined },
    ]);
  });

  it("et avbrutt vindu er ok med null innstillinger", async () => {
    ipc.answers.set("settings_pick_save_folder", async () => null);
    expect(await api.settingsPickSaveFolder()).toEqual({
      ok: true,
      settings: null,
    });
  });

  it("en avvisning kommer som { ok: false, error } med koden — siden sier hvilken regel", async () => {
    ipc.answers.set("settings_pick_save_folder", () =>
      Promise.reject({
        code: "validation",
        message: "validation: save_folder_is_a_package: …",
      }),
    );
    const answer = await api.settingsPickSaveFolder();
    expect(answer).toEqual({
      ok: false,
      error: "validation: save_folder_is_a_package: …",
    });
    expect(toasts).toEqual([]);
  });

  it("pickFolder og webviewets eget mappevindu er borte", () => {
    expect(
      (api as unknown as Record<string, unknown>).pickFolder,
    ).toBeUndefined();
  });
});

describe("innstillingsprofilen — Rust åpner vinduet, ingen sti sendes (A1)", () => {
  // Før valgte webviewet fila i sitt eget vindu og sendte STIEN; et
  // kompromittert webview kunne sendt en hvilken som helst sti uten å vise noe
  // vindu. Nå åpner kommandoen selv lagre-/åpne-vinduet, og shimmen sender
  // INGENTING — det er hele poenget, og det denne blokka pinner.

  it("eksporten sender ingen argumenter, og et avbrutt vindu er false", async () => {
    ipc.answers.set("settings_export_profile", async () => false);
    expect(await api.settingsExportProfile()).toBe(false);
    ipc.answers.set("settings_export_profile", async () => true);
    expect(await api.settingsExportProfile()).toBe(true);
    expect(callsTo("settings_export_profile")).toEqual([
      { cmd: "settings_export_profile", args: undefined },
      { cmd: "settings_export_profile", args: undefined },
    ]);
  });

  it("importen sender ingen argumenter, og et avbrutt vindu er null", async () => {
    ipc.answers.set("settings_import_profile", async () => null);
    expect(await api.settingsImportProfile()).toBeNull();
    expect(callsTo("settings_import_profile")).toEqual([
      { cmd: "settings_import_profile", args: undefined },
    ]);
  });

  it("en avvisning når kortet, som den er — kortet sier selv hva som gikk galt", async () => {
    const refusal = {
      code: "io",
      message: "io error: Permission denied (os error 13)",
    };
    ipc.answers.set("settings_export_profile", () => Promise.reject(refusal));
    await expect(api.settingsExportProfile()).rejects.toEqual(refusal);
    // Ingen ekstra toast fra shimmen: kortet viser sin egen.
    expect(toasts).toEqual([]);
  });

  it("de gamle sti-kommandoene og webviewets egne profilvinduer er borte", () => {
    const shim = readFileSync(join(repoRoot, "app/lib/api-shim.ts"), "utf8");
    for (const gone of [
      "settings_export_to_file",
      "settings_import_from_file",
      "pickSettingsFile",
      "pickSavePath",
    ]) {
      expect(shim).not.toContain(gone);
    }
    expect(
      (api as unknown as Record<string, unknown>).pickSavePath,
    ).toBeUndefined();
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

describe("ingen vei til dialog-pluginen fra webviewet (PR-F)", () => {
  const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
  // Nålene er stavet i to halvdeler så denne fila ikke treffer seg selv.
  const needles = ["@tauri-apps/plugin" + "-dialog", "plugin:" + "dialog"];

  function sources(dir: string): string[] {
    return readdirSync(dir).flatMap((name) => {
      const full = join(dir, name);
      if (name === "node_modules" || name === "bindings") return [];
      if (statSync(full).isDirectory()) return sources(full);
      return /\.(ts|tsx|js|mjs)$/.test(name) ? [full] : [];
    });
  }

  it("ingen kilde i app/, legacy/ eller e2e/ nevner dialog-pluginen", () => {
    const files = [
      ...sources(join(repo, "app")),
      ...sources(join(repo, "legacy")),
      ...sources(join(repo, "e2e")),
    ];
    expect(files.length).toBeGreaterThan(50);
    const offenders = files.filter((f) => {
      const src = readFileSync(f, "utf8");
      return needles.some((n) => src.includes(n));
    });
    expect(offenders.map((f) => relative(repo, f))).toEqual([]);
  });

  it("package.json og låsfila har ikke lenger dialog-pluginen", () => {
    const pkg = JSON.parse(readFileSync(join(repo, "package.json"), "utf8"));
    expect(pkg.dependencies?.[needles[0]]).toBeUndefined();
    expect(pkg.devDependencies?.[needles[0]]).toBeUndefined();
    const lock = readFileSync(join(repo, "package-lock.json"), "utf8");
    expect(lock).not.toContain(needles[0]);
  });
});
