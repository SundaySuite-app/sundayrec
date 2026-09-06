import { test, expect, type Page } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  recordingRow,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";
import type { TrashEntry } from "../legacy/bindings/TrashEntry";

// BIBLIOTEK — jobb nr. 2, sett utenfra. Nytt i P3, uten en legacy-motpart.
//
// Tabellen bak radene er node-testet (`app/pages/library/library-core.ts`).
// Det dette nivået legger til er SKJØTEN og de fire beslutningene canvasens
// sett 3 er godkjent på:
//
//   1. Papirkurven har ALLTID en inngang — også når den er tom. Det er atlaset
//      §5, funn 9, og den ene tilstanden som ikke engang var fotograferbar.
//   2. Slett spør ikke. Angre er sikkerheten.
//   3. «Slett nå» og «Tøm papirkurven» er de eneste permanente handlingene, og
//      de eneste med en dialog — med RØD SEKUNDÆR og AVBRYT på Enter.
//   4. Tellelinja beskriver radene som står, ikke arkivet.

const ROWS = [
  recordingRow({
    id: "rec-a",
    file_path: "/Users/test/Opptak/2026-08-09 Bønnemøte.mp3",
    started_at: 1_754_700_000_000,
    created_at: 1_754_700_000_000,
    duration_ms: 900_000,
  }),
  recordingRow({
    id: "rec-b",
    file_path: "/Users/test/Opptak/2026-08-02 Gudstjeneste.mp3",
    started_at: 1_754_100_000_000,
    created_at: 1_754_100_000_000,
    duration_ms: 5_400_000,
  }),
];

/** Én papirkurv-oppføring, i formen `trash_list` faktisk svarer med. Typed as
 *  the GENERATED `TrashEntry` binding — a Rust rename of any field must fail
 *  `npm run typecheck` here. */
function trashEntry(over: Partial<TrashEntry> = {}): TrashEntry {
  return {
    id: "t1",
    originalPath: "/Users/test/Opptak/2026-07-26 Gudstjeneste.mp3",
    trashedPath: "/Users/test/Opptak/.sundayrec-trash/x.mp3",
    name: "2026-07-26 Gudstjeneste.mp3",
    // Sju dager siden ⇒ 23 dager igjen av de 30.
    deletedAt: Date.now() - 7 * 24 * 60 * 60 * 1000,
    related: [],
    byteSize: 86_000_000,
    ...over,
  };
}

/**
 * `localStorage`-nøkkelen den fiktive papirkurven lever under — samme mønster
 * som harness.ts's fiktive sqlite-rad for innstillinger.
 *
 * Et `window.*`-globalt nulles av `page.reload()`; `localStorage` gjør det
 * IKKE. Det er nøyaktig forskjellen F2-A-F handler om: en «Slett» som bare
 * flyttet raden i rendererens eget minne ville sett riktig ut helt til siden
 * lastes på nytt. `library.spec.ts`s reload-test nedenfor er derfor avhengig
 * av at fikstur-papirkurven overlever en reload akkurat som en ekte backend
 * ville gjort — et JS-global ville gjort testen grønn av en gal grunn.
 */
const TRASH_DB_KEY = "__e2e.library.trashDb";

/** Papirkurven som en LISTE fikstursiden kan endre, med tellere på kommandoene
 *  som endrer den. Se `history.spec.ts` for hvorfor en statisk `trash_move`
 *  ikke holder her: skallet leser lista på nytt etter enhver endring. */
const TRASH_STORE: Fixtures = {
  trash_list: fn(
    `() => JSON.parse(localStorage.getItem(${JSON.stringify(TRASH_DB_KEY)}) || "[]")`,
  ),
  trash_move: fn(`(args) => {
    const key = ${JSON.stringify(TRASH_DB_KEY)};
    const list = JSON.parse(localStorage.getItem(key) || "[]");
    const moved = args.paths.map((p, i) => ({
      id: "t" + (list.length + i), originalPath: p, trashedPath: "/tmp/trash/x",
      name: p.split("/").pop(), deletedAt: Date.now(), related: [], byteSize: 1000,
    }));
    localStorage.setItem(key, JSON.stringify([...list, ...moved]));
    (window.__E2E_TRASHED__ ||= []).push(...args.paths);
    return moved;
  }`),
  trash_restore: fn(`(args) => {
    const key = ${JSON.stringify(TRASH_DB_KEY)};
    const list = JSON.parse(localStorage.getItem(key) || "[]");
    const at = list.findIndex((e) => e.id === args.id);
    const gone = at >= 0 ? list.splice(at, 1)[0] : null;
    localStorage.setItem(key, JSON.stringify(list));
    (window.__E2E_RESTORED__ ||= []).push(args.id);
    return gone ?? { id: args.id, originalPath: "", trashedPath: "", name: "",
                     deletedAt: Date.now(), related: [], byteSize: 0 };
  }`),
  trash_purge: fn(`(args) => {
    const key = ${JSON.stringify(TRASH_DB_KEY)};
    const list = JSON.parse(localStorage.getItem(key) || "[]");
    const keep = args.ids.length
      ? list.filter((e) => !args.ids.includes(e.id))
      : [];
    (window.__E2E_PURGED__ ||= []).push(args.ids);
    localStorage.setItem(key, JSON.stringify(keep));
    return list.length - keep.length;
  }`),
};

/** Legg oppføringene i den delte papirkurven FØR skallet leser den — i det
 *  SAMME `localStorage`-laget `TRASH_STORE` leser fra (se `TRASH_DB_KEY`). */
async function seedTrash(
  page: Page,
  entries: Record<string, unknown>[],
): Promise<void> {
  await page.addInitScript(
    ({ key, seed }: { key: string; seed: unknown }) => {
      window.localStorage.setItem(key, JSON.stringify(seed));
    },
    { key: TRASH_DB_KEY, seed: entries },
  );
}

async function openLibrary(page: Page, fixtures: Fixtures): Promise<void> {
  await boot(page, { fixtures, settings: SETTLED_SETTINGS, goto: "search" });
  await expect(page.getByTestId("main")).toHaveAttribute("data-page", "edit");
}

test.describe("bibliotek — lista", () => {
  test("tellelinja beskriver radene som står, ikke arkivet", async ({
    page,
  }) => {
    // 15 min + 1 t 30 min = 1 t 45 min. Summen er over ØKTENE: en økt med
    // kamera er to rader i basen, og legacys statistikklinje teller den
    // dobbelt (se `library-core.test.ts`).
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: ROWS,
      trash_list: [],
    });
    await expect(page.getByTestId("library-sub")).toHaveText(
      "Opptak: 2 · 1 t 45 min",
    );
  });

  test("raden dateres på starttidspunktet, ikke på når raden ble skrevet", async ({
    page,
  }) => {
    // `timestamp` er `created_at ?? started_at`, altså når gudstjenesten var
    // FERDIG. Med klokkeslettet i radens tittel er det ikke en unøyaktighet,
    // det er feil tid — her halvannen time feil. Shimmen bærer `startedAt`
    // videre nettopp for dette (P3, additivt).
    const started = Date.UTC(2026, 7, 16, 12, 0, 0);
    const written = started + 90 * 60 * 1000;
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: [
        recordingRow({
          id: "rec-sunday",
          file_path: "/Users/test/Opptak/2026-08-16 Gudstjeneste.mp3",
          started_at: started,
          created_at: written,
          duration_ms: 90 * 60 * 1000,
        }),
      ],
      trash_list: [],
    });

    // Klokkeslettene regnes ut I SIDEN, med appens eget språk: en literal her
    // ville testet hvilken tidssone kjøreren står i, ikke hvilket felt raden
    // dateres på.
    const [startClock, writtenClock] = await page.evaluate(
      ([a, b]) =>
        [a, b].map((ms) =>
          new Date(ms).toLocaleTimeString("no", {
            hour: "2-digit",
            minute: "2-digit",
          }),
        ),
      [started, written] as const,
    );

    const when = page.getByTestId("library-row-when");
    // Året er med — `intlParts`' `dateLong` har det ikke, og «søndag 16.
    // august» er to forskjellige gudstjenester i en menighet som har brukt
    // appen i to sesonger.
    await expect(when).toContainText("august 2026");
    await expect(when).toContainText(startClock);
    await expect(when).not.toContainText(writtenClock);
  });

  test("en ukjent varighet sier «—», ikke «0 min»", async ({ page }) => {
    // `rowToEntry` gjør en manglende `duration_ms` til 0, og «0 min» er en
    // påstand vi ikke kan stå for. WKWebView-proben i P2 fant nøyaktig den
    // setningen på eierens egen maskin.
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: [recordingRow({ duration_ms: null })],
      trash_list: [],
    });
    await expect(page.getByTestId("library-row-span")).toHaveText("—");
  });

  test("et opptak på under et minutt sier «Under 1 min», ikke «0 min»", async ({
    page,
  }) => {
    // WKWebView-probens funn på eierens EGEN profil: fem testopptak fra
    // Qu-5-runden var kortere enn et halvt minutt, og `spanOfSeconds` runder
    // til nærmeste minutt. «0 min» er da kjent OG usant — den samme setningen
    // P2 fjernet fra «Siste opptak»-kortet, med motsatt årsak.
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: [recordingRow({ duration_ms: 20_000 })],
      trash_list: [],
    });
    await expect(page.getByTestId("library-row-span")).toHaveText(
      "Under 1 min",
    );
    await expect(page.getByTestId("library-sub")).toHaveText(
      "Opptak: 1 · Under 1 min",
    );
  });

  test("et opptak med kamera er ÉN rad, med en Video-brikke", async ({
    page,
  }) => {
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: [
        recordingRow({
          id: "rec-v",
          file_path: "/Users/test/Opptak/2026-08-16 Gudstjeneste.mp4",
        }),
        recordingRow({
          id: "rec-a",
          file_path: "/Users/test/Opptak/2026-08-16 Gudstjeneste.wav",
        }),
      ],
      trash_list: [],
    });
    await expect(page.getByTestId("library-row")).toHaveCount(1);
    await expect(page.getByTestId("library-row-video")).toHaveText("Video");
  });
});

test.describe("bibliotek — papirkurvens inngang", () => {
  test("står der også når kurven er tom", async ({ page }) => {
    // Atlaset §5, funn 9: `refreshTrashButton()` i legacy setter
    // `display:none` på «Papirkurv» når `trash_list` er tom, og lukker
    // visningen hvis den står åpen. En frivillig som slettet noe i går og leter
    // etter det i dag finner da ingen dør.
    //
    // MUTASJONSPRØVEN: gjør lenken betinget av `inTrash > 0` i `Foot`
    // (`LibraryPage.tsx`) og denne blir rød.
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: ROWS,
      trash_list: [],
    });
    const link = page.getByTestId("library-trash-open");
    await expect(link).toBeVisible();
    await expect(link).toHaveText("Papirkurven er tom");

    await link.click();
    await expect(page.getByTestId("main")).toHaveAttribute("data-tab", "trash");
    await expect(page.getByTestId("trash-empty")).toBeVisible();
    // Skinnen står fortsatt på BIBLIOTEK — papirkurven er et sted INNE i
    // biblioteket, ikke et fjerde sted i appen.
    await expect(page.getByTestId("nav-edit")).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  test("teller når det ligger noe der", async ({ page }) => {
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: ROWS,
      trash_list: [trashEntry(), trashEntry({ id: "t2" })],
    });
    await expect(page.getByTestId("library-trash-open")).toHaveText(
      "Papirkurv (2)",
    );
  });
});

test.describe("papirkurven", () => {
  test("«Legg tilbake» går til bakenden og tømmer raden", async ({ page }) => {
    await seedTrash(page, [trashEntry()]);
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      ...TRASH_STORE,
      recordings_list: ROWS,
    });
    await page.getByTestId("library-trash-open").click();

    await expect(page.getByTestId("trash-row")).toHaveCount(1);
    // 30 dagers frist minus sju dager i kurven.
    await expect(page.getByTestId("trash-row-due")).toHaveText(
      "Slettes om 23 dager",
    );

    await page.getByTestId("trash-row-restore").click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_RESTORED__))
      .toEqual(["t1"]);
    await expect(page.getByTestId("trash-empty")).toBeVisible();
  });

  test("«Slett nå» er den ENE handlingen som spør — med rød sekundær", async ({
    page,
  }) => {
    await seedTrash(page, [trashEntry()]);
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      ...TRASH_STORE,
      recordings_list: ROWS,
    });
    await page.getByTestId("library-trash-open").click();
    await page.getByTestId("trash-row-purge").click();

    const dialog = page.getByTestId("dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute("data-danger", "true");
    await expect(page.getByTestId("dialog-title")).toContainText(
      "2026-07-26 Gudstjeneste.mp3",
    );
    // Canvasens sett 7: en farlig dialog har rød SEKUNDÆR, aldri rød primær —
    // og AVBRYT er det Enter treffer.
    await expect(page.getByTestId("dialog-ok")).toHaveAttribute(
      "data-variant",
      "danger",
    );
    await expect(page.getByTestId("dialog-cancel")).toBeFocused();

    // Avbryt ⇒ ingenting skjedde.
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    expect(await page.evaluate(() => (window as any).__E2E_PURGED__)).toBe(
      undefined,
    );
    await expect(page.getByTestId("trash-row")).toHaveCount(1);

    // …og med et JA når den beskjeden fram.
    await page.getByTestId("trash-row-purge").click();
    await page.getByTestId("dialog-ok").click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_PURGED__))
      .toEqual([["t1"]]);
    await expect(page.getByTestId("trash-empty")).toBeVisible();
  });

  test("«Tøm papirkurven» navngir antallet, og tømmer alt", async ({
    page,
  }) => {
    // «Slett hele papirkurven?» gir ingen følelse av omfang, og forskjellen på
    // å miste 2 og 200 opptak er hele beslutningen.
    await seedTrash(page, [trashEntry(), trashEntry({ id: "t2" })]);
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      ...TRASH_STORE,
      recordings_list: ROWS,
    });
    await page.getByTestId("library-trash-open").click();
    await expect(page.getByTestId("trash-row")).toHaveCount(2);

    await page.getByTestId("trash-empty-all").click();
    await expect(page.getByTestId("dialog-title")).toHaveText(
      "Slett 2 opptak for godt?",
    );
    await expect(page.getByTestId("dialog")).toHaveAttribute(
      "data-danger",
      "true",
    );
    await page.getByTestId("dialog-ok").click();

    // Tom `ids` betyr «tøm alt» i Rust — ikke en liste skjermen bygger selv.
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_PURGED__))
      .toEqual([[]]);
    await expect(page.getByTestId("trash-empty")).toBeVisible();
    await expect(page.getByTestId("trash-empty-all")).toHaveCount(0);
  });

  test("«Tilbake til biblioteket» går tilbake dit", async ({ page }) => {
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: ROWS,
      trash_list: [],
    });
    await page.getByTestId("library-trash-open").click();
    await expect(page.getByTestId("main")).toHaveAttribute("data-tab", "trash");
    await page.getByTestId("trash-back").click();
    await expect(page.getByTestId("main")).not.toHaveAttribute(
      "data-tab",
      "trash",
    );
    await expect(page.getByTestId("library-row")).toHaveCount(2);
  });
});

// ── F2-T3: tastatursnarveier ─────────────────────────────────────────────────
//
// `decideShortcut` er node-testet som en tabell. Det dette nivået beviser er
// SKJØTEN: at ⌘F/Ctrl+F faktisk flytter fokus i en ekte nettleser, og at et
// vanlig mellomrom fortsatt bare er et mellomrom når det skrives i feltet
// snarveien selv peker på.
test.describe("F2-T3: tastatursnarveier i biblioteket", () => {
  test("⌘F og Ctrl+F fokuserer og markerer søket", async ({ page }) => {
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: ROWS,
      trash_list: [],
    });
    const search = page.getByTestId("library-search");
    await search.fill("bønn");
    // Fjern fokus fra feltet FØRST — ellers beviser testen ingenting om at
    // snarveien er den som FLYTTER det dit.
    await page.getByTestId("library-open-file").focus();
    await expect(search).not.toBeFocused();

    await page.keyboard.press("Meta+f");
    await expect(search).toBeFocused();
    // «marker innholdet»: hele verdien er valgt, ikke bare fokusert.
    expect(
      await search.evaluate((el: HTMLInputElement) => [
        el.selectionStart,
        el.selectionEnd,
        el.value,
      ]),
    ).toEqual([0, 4, "bønn"]);

    // Ctrl+F gjør akkurat det samme — ingen platform-sperre i tabellen (kun i
    // HVILKEN hint-tekst placeholderen viser).
    await page.getByTestId("library-open-file").focus();
    await expect(search).not.toBeFocused();
    await page.keyboard.press("Control+f");
    await expect(search).toBeFocused();
  });

  test("placeholderen bærer den ekte snarveien, ikke en hardkodet setning", async ({
    page,
  }) => {
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: ROWS,
      trash_list: [],
    });
    // Chromium her rapporterer «MacIntel»/mac-UA uansett vertens OS — samme
    // kilde `useGlobalShortcuts` selv IKKE bruker (den godtar begge
    // modifikatorene), men som placeholderens hint-tekst gjør.
    await expect(page.getByTestId("library-search")).toHaveAttribute(
      "placeholder",
      /⌘F|Ctrl\+F/,
    );
  });

  test("Space i søkefeltet skriver et mellomrom — ingen snarvei stjeler det", async ({
    page,
  }) => {
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: ROWS,
      trash_list: [],
    });
    const search = page.getByTestId("library-search");
    await search.click();
    await page.keyboard.type("a b");
    await expect(search).toHaveValue("a b");
  });
});

// ── F2-A-F: papirkurven skjuler raden, ikke bare rendererens minne ─────────
//
// Funnet: biblioteket filtrerte bare ved LASTING (`getHistory()`s egen
// `trash_list`-join), aldri reaktivt mot et signal. En «Slett» som bare
// fjernet raden lokalt ville sett riktig ut i akkurat den samme testen som
// øvrige spec-er her kjører — helt til siden lastes på nytt. Derfor
// `page.reload()` under, ikke bare en ny render: det er den ENE handlingen
// som skiller «flyttet i en fiktiv backend» fra «bare pyntet i DOM-en», og
// `TRASH_STORE` er `localStorage`-lagret (se `TRASH_DB_KEY`) nettopp for at
// reloaden tester APPEN og ikke fikstur-seedingen.
test.describe("F2-A-F: en slettet rad overlever en omstart", () => {
  test("slett → reload (omstart) → raden er fortsatt borte; angre → raden er tilbake, også etter enda en reload", async ({
    page,
  }) => {
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      ...TRASH_STORE,
      recordings_list: ROWS,
    });
    await expect(page.getByTestId("library-row")).toHaveCount(2);

    await page
      .getByTestId("library-row")
      .filter({ hasText: "Bønnemøte" })
      .getByTestId("library-row-delete")
      .click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_TRASHED__))
      .toEqual(["/Users/test/Opptak/2026-08-09 Bønnemøte.mp3"]);
    await expect(page.getByTestId("library-row")).toHaveCount(1);

    // Omstarten. `?goto=search` står fortsatt i URL-en etter en reload, så
    // appen lander tilbake på biblioteket akkurat som ved en ekte kald start.
    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    await expect(page.getByTestId("main")).toHaveAttribute("data-page", "edit");

    // Raden er FORTSATT borte — historikkraden består (F1-M2), men den skal
    // ikke tilby «Rediger» mot en fil som ligger i papirkurven.
    await expect(page.getByTestId("library-row")).toHaveCount(1);
    await expect(page.getByTestId("main")).not.toContainText("Bønnemøte");

    // Angre, fra papirkurv-visningen inne i biblioteket — raden kommer
    // tilbake med en gang.
    await page.getByTestId("library-trash-open").click();
    await expect(page.getByTestId("trash-row")).toHaveCount(1);
    await page.getByTestId("trash-row-restore").click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_RESTORED__))
      .toEqual(["t0"]);
    await page.getByTestId("trash-back").click();
    await expect(page.getByTestId("library-row")).toHaveCount(2);
    await expect(page.getByTestId("main")).toContainText("Bønnemøte");

    // Og symmetrisk: den gjenopprettede raden blir stående etter ENDA en
    // omstart, ikke bare i den samme fanen som klikket «Legg tilbake».
    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    await expect(page.getByTestId("library-row")).toHaveCount(2);
    await expect(page.getByTestId("main")).toContainText("Bønnemøte");
  });
});

// ── F2-9: en fil path_guard ikke lenger kan løse opp ────────────────────────

test.describe("REDIGERING på en fil som ikke lenger er der", () => {
  test("«Fant ikke fila», med en vei til papirkurven — ikke den generiske korrupt-teksten", async ({
    page,
  }) => {
    // ⚠️ FUNNET, og det er ekte. FØR F2-9 viste denne skjermen «Formatet
    // støttes kanskje ikke, eller fila er skadet» uansett HVORFOR åpningen
    // feilet — sant om ingen av delene når grunnen var at fila lå i
    // papirkurven. `path_guard::checked_input_file` sin egen feilmelding
    // («cannot resolve path …») er ORDRETT det en flyttet fil produserer;
    // fixturen under er den meldingen, ikke en forenkling av den.
    //
    // MUTASJONSPRØVEN: fjern `notFound`-grenen fra `loader.ts` (la
    // `loadError.value` alltid bli `"unreadable"`), og de tre `toContainText`
    // -assertionene på banneret blir røde.
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: [
        recordingRow({
          id: "rec-gone",
          file_path: "/Users/test/Opptak/2026-08-09 Bønnemøte.mp3",
        }),
      ],
      editor_load_recording: fn(`() => {
        throw new Error(
          "validation: cannot resolve path /Users/test/Opptak/2026-08-09 Bønnemøte.mp3: No such file or directory (os error 2)",
        );
      }`),
    });

    await page.getByTestId("library-row-edit").click();

    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "error",
    );
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-reason",
      "not_found",
    );
    const banner = page.getByTestId("editor-load-error");
    await expect(banner).toContainText("Fant ikke fila");
    await expect(banner).toContainText("Den kan ligge i papirkurven.");
    // …og ikke det generiske «formatet støttes kanskje ikke, eller fila er
    // skadet» — aktivt misvisende om en fil som bare ligger i papirkurven.
    await expect(banner).not.toContainText("skadet");

    // Den nye handlingen går til papirkurven, samme mål som retensjonens
    // toast (`retention.spec.ts`) navigerer til.
    await page.getByTestId("editor-load-error-trash").click();
    await expect(page.getByTestId("main")).toHaveAttribute("data-page", "edit");
    await expect(page.getByTestId("main")).toHaveAttribute("data-tab", "trash");
  });

  test("en fil som genuint ikke lar seg lese viser fortsatt den generiske teksten", async ({
    page,
  }) => {
    // Kontrasten: en feil som IKKE er «cannot resolve path»/`file_not_found`
    // skal fortsatt lande på den gamle, generiske teksten — F2-9 la til en
    // gren, den fjernet ikke den andre.
    await openLibrary(page, {
      ...BOOT_FIXTURES,
      recordings_list: [
        recordingRow({
          id: "rec-corrupt",
          file_path: "/Users/test/Opptak/2026-08-09 Bønnemøte.mp3",
        }),
      ],
      editor_load_recording: fn(`() => {
        throw new Error("recording error: ffprobe found no audio or video stream");
      }`),
    });

    await page.getByTestId("library-row-edit").click();

    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-reason",
      "unreadable",
    );
    const banner = page.getByTestId("editor-load-error");
    await expect(banner).toContainText(
      "Formatet støttes kanskje ikke, eller fila er skadet.",
    );
    await expect(page.getByTestId("editor-load-error-trash")).toHaveCount(0);
  });
});
