/**
 * Avspillingsmotoren, sett fra lyd-steget (B5b): `window.api.editorListen*` som en logg.
 *
 * Steget snakker med Rust i fire kommandoer og én hendelseskanal. Her er de en liste over
 * hva som ble bedt om, med porter testen kan åpne i den rekkefølgen den vil (kjeden er det
 * som kan ta et minutt i virkeligheten), og en `emit` som sier det motoren ville sagt.
 *
 * Bare for tester.
 */

import type { EditorListenEvent } from "@legacy/bindings/EditorListenEvent";

export type ListenCall =
  | { cmd: "open"; token: string; listen: Record<string, unknown> }
  | { cmd: "chain"; token: string; listen: Record<string, unknown> }
  | { cmd: "transport"; action: Record<string, unknown> }
  | { cmd: "close" };

export interface FakeListen {
  api: Record<string, unknown>;
  log: ListenCall[];
  /** `transport`-handlingene, i rekkefølge. */
  actions(): Array<Record<string, unknown>>;
  /** Kjede-forespørslene, i rekkefølge. */
  chains(): Array<Record<string, unknown>>;
  /** Sagt av motoren (`editor-listen`). */
  emit(event: EditorListenEvent): void;
  /** Antall som lytter på `editor-listen`. */
  listeners(): number;
  /** Hold neste kjede-svar tilbake til testen åpner det. */
  holdChain(): (ok: boolean) => void;
  /** La åpningen mislykkes. */
  failOpen(): void;
}

export function fakeListen(): FakeListen {
  const log: ListenCall[] = [];
  let handlers: Array<(p: unknown) => void> = [];
  const held: Array<Promise<boolean>> = [];
  let openFails = false;
  const api = {
    editorListenOpen: (token: string, listen: Record<string, unknown>) => {
      log.push({ cmd: "open", token, listen });
      return Promise.resolve(
        openFails
          ? { ok: false as const, error: "no output device" }
          : { ok: true as const, programSecs: 3500 },
      );
    },
    editorListenChain: (token: string, listen: Record<string, unknown>) => {
      log.push({ cmd: "chain", token, listen });
      return held.shift() ?? Promise.resolve(true);
    },
    editorListenTransport: (action: Record<string, unknown>) => {
      log.push({ cmd: "transport", action });
      return Promise.resolve();
    },
    editorListenClose: () => {
      log.push({ cmd: "close" });
      return Promise.resolve();
    },
    on: (channel: string, fn: (p: unknown) => void) => {
      if (channel !== "editor-listen") return () => {};
      handlers.push(fn);
      return () => {
        handlers = handlers.filter((h) => h !== fn);
      };
    },
  };
  return {
    api,
    log,
    actions: () =>
      log.flatMap((c) => (c.cmd === "transport" ? [c.action] : [])),
    chains: () => log.flatMap((c) => (c.cmd === "chain" ? [c.listen] : [])),
    emit: (e) => handlers.slice().forEach((h) => h(e)),
    listeners: () => handlers.length,
    holdChain: () => {
      let release!: (ok: boolean) => void;
      held.push(new Promise<boolean>((r) => (release = r)));
      return release;
    },
    failOpen: () => {
      openFails = true;
    },
  };
}
