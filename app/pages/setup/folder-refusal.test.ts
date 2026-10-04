/**
 * «Hvorfor ble ikke mappa lagret?» — skjøten mellom bakendens regel og
 * toasten.
 *
 * ## Hva som bevises
 *
 * Bakenden avviser en NY opptaksmappe med en kode
 * (`vet_new_save_folder` i `src-tauri/src/commands/recordings_open.rs`).
 * Toasten kan bare si hvorfor hvis tre ledd er enige:
 *
 *   1. Rust sender koden (lest rett fra kilden — en ny regel i Rust uten
 *      setning her blir rød, ikke en stille «Kunne ikke lagre»);
 *   2. koden kommer fram til siden — fra mappevinduet Rust åpner
 *      (`settingsPickSaveFolder`, svaret `{ ok: false, error }`), og fra en
 *      avvist lagring (`lastSaveFailureCode` i `state/settings.ts`);
 *   3. `folderRefusalMessage` har en setning for den.
 *
 * Det er skjøtefeilens form: tre ledd, hvert grønt for seg.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { errorCode } from "@lib/error-code-core";

import { setLocale, t } from "../../i18n";
import {
  lastSaveFailureCode,
  saveSettingsDebounced,
} from "../../state/settings";
import { folderRefusalMessage } from "./folder-refusal";

const ROOT = join(import.meta.dirname, "../../..");

/** Hver `save_folder_*`-kode vet-funksjonen kan svare med, lest fra Rust. */
function rustSaveFolderCodes(): string[] {
  const src = readFileSync(
    join(ROOT, "src-tauri/src/commands/recordings_open.rs"),
    "utf8",
  );
  const codes = [...src.matchAll(/"(save_folder_[a-z_]+):/g)].map((m) => m[1]);
  return [...new Set(codes)].sort();
}

/** Et `window.api` med bare den ene kommandoen lagringen bruker. */
function withSave(save: () => Promise<unknown>): void {
  (globalThis as unknown as { window: unknown }).window = {
    api: { saveSettings: save },
  };
}

beforeAll(async () => {
  await setLocale("no");
});

afterEach(() => {
  delete (globalThis as unknown as { window?: unknown }).window;
});

describe("folderRefusalMessage", () => {
  it("lesingen av Rust-kilden finner de fem reglene", () => {
    // En lesing som fant null koder ville gjort alt under grønt.
    expect(rustSaveFolderCodes()).toEqual([
      "save_folder_app_data",
      "save_folder_invalid",
      "save_folder_is_a_package",
      "save_folder_protected",
      "save_folder_too_broad",
    ]);
  });

  it.each(rustSaveFolderCodes())("har en egen setning for %s", (code) => {
    const message = folderRefusalMessage(code);
    expect(message).toBeTruthy();
    expect(message).not.toBe(t("general.saveFailed"));
  });

  it("de fem setningene er fem forskjellige", () => {
    const messages = rustSaveFolderCodes().map(folderRefusalMessage);
    expect(new Set(messages).size).toBe(messages.length);
  });

  it("en annen feil får den vanlige teksten (null)", () => {
    expect(folderRefusalMessage("")).toBeNull();
    expect(folderRefusalMessage("database_locked")).toBeNull();
    expect(folderRefusalMessage("recordings_folder_missing")).toBeNull();
  });
});

describe("mappevinduet", () => {
  it("en avvisning fra mappevinduet når fram til setningen", () => {
    // `settingsPickSaveFolder` svarer `{ ok: false, error }` med Rusts egen
    // tekst; `FolderPage` leser koden ut av den med `errorCode`.
    const error =
      "validation: save_folder_too_broad: the recordings folder cannot be the file system root, the home folder or a folder above it";
    expect(errorCode(error)).toBe("save_folder_too_broad");
    expect(folderRefusalMessage(errorCode(error))).toBe(
      t("app.setup.folder.refusedTooBroad"),
    );
    // Hver kode Rust kan svare med har sin setning den veien også.
    for (const code of rustSaveFolderCodes()) {
      expect(
        folderRefusalMessage(errorCode(`validation: ${code}: …`)),
      ).toBeTruthy();
    }
  });
});

describe("lagringen husker hvorfor den ble avvist", () => {
  it("koden fra et avvist settings_save når fram til setningen", async () => {
    withSave(() =>
      Promise.reject({
        code: "validation",
        message:
          "validation: save_folder_too_broad: the recordings folder cannot be the file system root, the home folder or a folder above it",
      }),
    );
    expect(await saveSettingsDebounced(0)).toBe(false);
    expect(lastSaveFailureCode()).toBe("save_folder_too_broad");
    expect(folderRefusalMessage(lastSaveFailureCode())).toBe(
      t("app.setup.folder.refusedTooBroad"),
    );
  });

  it("en lagring som landet glemmer den forrige avvisningen", async () => {
    withSave(() =>
      Promise.reject({
        code: "validation",
        message: "validation: save_folder_is_a_package: …",
      }),
    );
    expect(await saveSettingsDebounced(0)).toBe(false);
    expect(lastSaveFailureCode()).toBe("save_folder_is_a_package");
    withSave(() => Promise.resolve(true));
    expect(await saveSettingsDebounced(0)).toBe(true);
    expect(lastSaveFailureCode()).toBe("");
  });

  it("en feil uten kode gir ingen kode — og dermed den vanlige teksten", async () => {
    withSave(() =>
      Promise.reject(new Error("SQLITE_BUSY: database is locked")),
    );
    expect(await saveSettingsDebounced(0)).toBe(false);
    expect(folderRefusalMessage(lastSaveFailureCode())).toBeNull();
  });
});
