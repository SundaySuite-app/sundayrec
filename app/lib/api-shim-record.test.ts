/**
 * `startRecordingNow` — nøyaktig hva shimmen sender når noen trykker Start.
 *
 * ## Hvorfor
 *
 * Sikkerhetsfunn E1: shimmen spurte `plan_recording_opts` om HELE
 * opptaksoppsettet og sendte det rett tilbake til `start_recording`. Da var
 * `output_path` — fila opptakeren lager mappe til, tar opp i og skriver over —
 * en streng fra webviewet, og en kompromittert side kunne pekt et opptak på
 * hvilken som helst sti brukeren kan skrive til.
 *
 * Nå planlegger Rust selv, og shimmen sender bare de tre tingene som faktisk er
 * sidas å bestemme: navn, maks lengde og video av/på. Det denne fila pinner er
 * SØMMEN fra denne siden:
 *
 *   • ett kall, `start_recording`, med `{ request: { customName, maxMinutes,
 *     video } }` — de samme tre verdiene, med de samme omformingene, som det
 *     gamle `plan_recording_opts`-kallet sendte;
 *   • ingen nøkkel noe sted i argumentene som ser ut som en sti;
 *   • `plan_recording_opts` kalles ikke (kommandoen er slettet i Rust);
 *   • et nei fra motoren blir `{ ok: false, error }` med den samme teksten som
 *     før, så Opptak-sida viser den samme toasten.
 *
 * Rust-siden av den samme sømmen: `the_shims_payload_is_the_request` i
 * `src-tauri/src/commands/recorder.rs`, som deserialiserer nøyaktig disse
 * kroppene.
 *
 * Verdenen bygges som i `api-shim-files.test.ts`.
 */

import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

const ipc = vi.hoisted(() => ({
  calls: [] as Array<{ cmd: string; args: unknown }>,
  answers: new Map<string, () => Promise<unknown>>(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => {
    ipc.calls.push({ cmd, args });
    const answer = ipc.answers.get(cmd);
    return answer ? answer() : Promise.reject(new Error(`not mocked: ${cmd}`));
  },
  convertFileSrc: (p: string) => p,
  isTauri: () => true,
}));

type Api = {
  startRecordingNow: (
    opts: unknown,
  ) => Promise<{ ok?: boolean; error?: string }>;
};

let api: Api;
const toasts: Array<{ kind: string; msg: string }> = [];

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
});

beforeEach(() => {
  ipc.calls.length = 0;
  ipc.answers.clear();
  toasts.length = 0;
});

/** Every key, at any depth, in an IPC argument object. */
function allKeys(v: unknown): string[] {
  if (!v || typeof v !== "object") return [];
  return Object.entries(v as Record<string, unknown>).flatMap(([k, inner]) => [
    k,
    ...allKeys(inner),
  ]);
}

/** The ratchet's own name rule (`is_path_like` in
 *  `src-tauri/src/commands/path_ratchet.rs`), plus the sidecar extension. */
const PATH_SHAPED = /path|folder$|dir$|file$|separateaudio/i;

describe("startRecordingNow", () => {
  it("det Opptak-sida sender: ett kall, ingen navn, ingen grense, video av", async () => {
    ipc.answers.set("start_recording", async () => null);
    // Nøyaktig det `handleStart` sender med «Maks lengde» av og uten kamera.
    const res = await api.startRecordingNow({
      maxMinutes: undefined,
      videoEnabled: false,
    });
    expect(res).toEqual({ ok: true });
    expect(ipc.calls).toEqual([
      {
        cmd: "start_recording",
        args: {
          request: { customName: null, maxMinutes: null, video: false },
        },
      },
    ]);
  });

  it("navn, grense og video går fram med de samme omformingene som før", async () => {
    ipc.answers.set("start_recording", async () => null);
    await api.startRecordingNow({
      customName: "Høymesse – 1. søndag i advent",
      maxMinutes: 90,
      videoEnabled: true,
    });
    // Tomt navn → null (profilens mønster), `undefined` video → false: de
    // samme `|| null` / `!!` det gamle `plan_recording_opts`-kallet brukte.
    await api.startRecordingNow({ customName: "", maxMinutes: 0 });
    await api.startRecordingNow(undefined);
    expect(ipc.calls.map((c) => c.args)).toEqual([
      {
        request: {
          customName: "Høymesse – 1. søndag i advent",
          maxMinutes: 90,
          video: true,
        },
      },
      { request: { customName: null, maxMinutes: 0, video: false } },
      { request: { customName: null, maxMinutes: null, video: false } },
    ]);
  });

  it("sender aldri en sti — heller ikke når noen gir shimmen en", async () => {
    ipc.answers.set("start_recording", async () => null);
    // Den gamle formen: et helt opptaksoppsett, med en sti i. Shimmen skal
    // bare plukke de tre feltene den eier, og la resten ligge.
    await api.startRecordingNow({
      customName: "Søndag",
      maxMinutes: 60,
      videoEnabled: false,
      output_path: "/tmp/evil.mp3",
      outputPath:
        "C:\\Users\\x\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\a.cmd",
      saveFolder: "/tmp",
      separate_audio_format: "cmd",
    });
    expect(ipc.calls).toHaveLength(1);
    const keys = allKeys(ipc.calls[0]?.args);
    expect(keys).toEqual(["request", "customName", "maxMinutes", "video"]);
    expect(keys.filter((k) => PATH_SHAPED.test(k))).toEqual([]);
    expect(JSON.stringify(ipc.calls[0]?.args)).not.toMatch(/evil|Startup|tmp/);
  });

  it("spør ikke lenger plan_recording_opts — Rust planlegger selv", async () => {
    ipc.answers.set("start_recording", async () => null);
    ipc.answers.set("plan_recording_opts", async () => ({
      output_path: "/tmp/evil.mp3",
    }));
    await api.startRecordingNow({ videoEnabled: false });
    expect(ipc.calls.map((c) => c.cmd)).toEqual(["start_recording"]);
  });

  it("et nei fra motoren blir { ok: false, error } med den samme teksten som før", async () => {
    // `AppError` kommer over IPC som `{ code, message }`. Meldingen bærer den
    // granulære koden Opptak-sida oversetter (`nativeErrorSuffixFromText`).
    ipc.answers.set("start_recording", () =>
      Promise.reject({
        code: "validation",
        message: "validation: no_save_folder: no save folder could be resolved",
      }),
    );
    expect(await api.startRecordingNow({ videoEnabled: false })).toEqual({
      ok: false,
      error: "validation: no_save_folder: no save folder could be resolved",
    });
    // Shimmen toaster ikke selv på denne stien — sida gjør det, én gang.
    expect(toasts).toEqual([]);
  });
});
