import { expect, test, type Page } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  recordingRow,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";
import { editorFixtures, FILE } from "./editor-fixtures";

// TREFFFLATENE (V1/E4) — at bryteren og utfoldingsknappen kan TREFFES, og at
// ingen av dem stjeler naboens klikk.
//
// ## Hvorfor `elementFromPoint` og ikke `boundingBox`
//
// Grepet er et gjennomsiktig `::after` utenfor knappens egen boks. Det gjør
// flaten større UTEN å gjøre knappen større — som er hele poenget: raden i
// kontrollrommet skal fortsatt leses som én linje, og bryteren skal fortsatt
// se ut som en bryter.
//
// Men da ser `getBoundingClientRect()` — og dermed Playwrights `boundingBox()`
// — fortsatt 22 px. Den måler elementets boks, ikke hva som svarer på et
// trykk. Den ene målestokken som ser det samme som en finger gjør, er å spørre
// nettleseren hva som ligger på et punkt. Derfor er hver eneste påstand her
// `document.elementFromPoint`, og derfor er en `boundingBox`-basert versjon av
// denne testen verdiløs uansett hvor grønn den er.
//
// ## De to påstandene
//
//   1. POSITIVT: flaten rekker minst 40 px (bryter) / 44 px (utfolding) —
//      og BOKSEN står stille (22 px / ~30 px). Vokser boksen, er grepet gjort
//      feil vei.
//   2. NEGATIVT: hver eneste klikkbare ting på skjermen svarer fortsatt på sitt
//      EGET midtpunkt. En flate som rakk inn over naboen ville vært et verre
//      bytte enn den lille knappen den erstattet, og den feilen er usynlig i
//      alt annet enn nettopp denne målingen.
//
// MUTASJONSPRØVENE (kjørt, 2026-08-30):
//
//   - `content: none` på ett av de to pseudo-elementene → punkt 1 rød på ALLE
//     fem testene (målt 22,5 og 30,5 mot gulvene 40 og 44).
//   - `pointer-events: none` på `.toggle::after` → samme røde. Flaten som ikke
//     treffer er ingen flate.
//   - `left: -90px` på `.toggle::after` → punkt 2 rød, med den ene ekte naboen
//     som finnes: `adv-diag-delete → adv-diag-control-input`. Negativtesten er
//     altså ikke tom — den vet om det trangeste stedet i appen.
//
// Tallene her er også målt i en EKTE WKWebView (Swift-vert, samme probe): 44,5
// og 40,5. Se `docs/APP-SHELL.md` §«Treffflatene».
//
// F1-UX1/W2: ghost-knappene («Slett», «Vis i Finder», «Endre», …) og
// Bibliotekets rader lagt til med SAMME metode. De hadde ingen egen flate å
// finne feil i — målt gjennom hele appen (Opptak, Avansert, Bibliotek,
// editoren) er boksen alltid 40,3 px høy, husets faste knappepadding — men nå
// STÅR gulvet, i stedet for å være noe hver skjerm bare håpet.

/** Gulvene. 44 er fingergulvet; bryteren er 40 fordi bredden ER 40 og en
 *  treffflate som ikke er kvadratisk er en flate man bommer på skjevt. */
const TOGGLE_FLOOR = 40;
const EXPAND_FLOOR = 44;

/**
 * F1-UX1/W2: ghost-knappene («Slett», «Vis i Finder», «Ferdig», …) har INGEN
 * egen usynlig flate — målt gjennom hele appen (Opptak, Avansert, Bibliotek,
 * editoren, kvitteringen, overlegget) er boksen alt 40,3 px høy, husets faste
 * `.btn`-padding pluss linjehøyden. Samme gulv som bryteren, men her er det en
 * LÅS: neste gang noen strammer knappepaddingen for å få mer luft et sted,
 * skal denne testen si nei før en frivillig merker det.
 */
const GHOST_FLOOR = 40;

/**
 * Måleren har 0,5 px oppløsning, så en 40,0 px flate leses som 40,0 ± 0,5
 * avhengig av om kontrollens boks starter på et halvt piksel. Gulvet i
 * påstanden er derfor ett halvt piksel under det tallet designet lover — ikke
 * for å gi slakk, men fordi et strengere tall ville vært en påstand om
 * måleren og ikke om flaten.
 */
const PROBE_STEP = 0.5;

interface Hit {
  id: string;
  /** Elementets EGEN boks — den som ikke skal ha flyttet seg. */
  boxH: number;
  boxW: number;
  /** Så høyt/bredt det faktisk svarer på et trykk. */
  hitH: number;
  hitW: number;
}

/**
 * Mål treffflaten til hver kontroll som matcher `selector`.
 *
 * Alt skjer i ÉN `evaluate`: elementet rulles til midten av ruta (ellers måler
 * man på et punkt utenfor vinduet, og `elementFromPoint` svarer `null` på alt
 * der), og så vandrer proben utover fra midtpunktet til nettleseren svarer med
 * noe annet enn kontrollen selv.
 */
async function measure(page: Page, selector: string): Promise<Hit[]> {
  return page.evaluate(
    ({ sel, step }) => {
      const nodes = Array.from(document.querySelectorAll(sel)) as HTMLElement[];
      const out: Hit[] = [];
      for (const node of nodes) {
        if (node.getBoundingClientRect().width === 0) continue;
        // `Gate` slår av hele undertreet sitt med `inert` når bakenden ikke
        // finnes (e-post uten `smtp`-featuren). Da svarer `elementFromPoint`
        // med gaten og ikke kontrollen — som er nøyaktig meningen, og ikke noe
        // en treffflate kan eller skal gjøre noe med.
        if (node.closest("[inert]")) continue;
        node.scrollIntoView({ block: "center", inline: "center" });
        const r = node.getBoundingClientRect();
        const cx = r.left + r.width / 2;
        const cy = r.top + r.height / 2;
        const owns = (x: number, y: number) => {
          const hit = document.elementFromPoint(x, y);
          return !!hit && (hit === node || node.contains(hit));
        };
        /** Hvor langt proben kommer i én retning før noen andre svarer. */
        const reach = (dx: number, dy: number) => {
          let last = 0;
          for (let d = step; d <= 60; d += step) {
            if (!owns(cx + dx * d, cy + dy * d)) break;
            last = d;
          }
          return last;
        };
        out.push({
          id:
            node.dataset.testid ??
            `${node.tagName.toLowerCase()}@${Math.round(r.y)}`,
          boxH: Math.round(r.height * 100) / 100,
          boxW: Math.round(r.width * 100) / 100,
          hitH: reach(0, -1) + reach(0, 1),
          hitW: reach(-1, 0) + reach(1, 0),
        });
      }
      return out;
    },
    { sel: selector, step: PROBE_STEP },
  );
}

/**
 * NEGATIVTESTEN: ingen kontroll har mistet sitt eget midtpunkt.
 *
 * Bredt med vilje — ikke bare naboene til de to kontrollene som fikk en flate,
 * men ALT som kan klikkes på skjermen. Et pseudo-element er posisjonert, og
 * posisjonerte ting males (og treffes) over sine ikke-posisjonerte søsken; hvem
 * som faktisk havner under er et spørsmål om DOM-rekkefølge og layout, ikke om
 * hva den som skrev CSS-en hadde i tankene. Så spørsmålet stilles til hele
 * skjermen.
 */
async function nobodyStolen(page: Page): Promise<void> {
  const stolen = await page.evaluate(() => {
    const name = (el: Element): string => {
      const owner = (el.closest("[data-testid]") as HTMLElement | null)?.dataset
        .testid;
      return owner ?? el.tagName.toLowerCase();
    };
    const nodes = Array.from(
      document.querySelectorAll("button, select, textarea, input, a[href]"),
    ) as HTMLElement[];
    const out: string[] = [];
    for (const node of nodes) {
      if (node.getBoundingClientRect().width === 0) continue;
      // Se `measure`: et inert undertre SKAL ikke svare på et trykk.
      if (node.closest("[inert]")) continue;
      node.scrollIntoView({ block: "center", inline: "center" });
      const r = node.getBoundingClientRect();
      const hit = document.elementFromPoint(
        r.left + r.width / 2,
        r.top + r.height / 2,
      );
      if (!hit) {
        out.push(`${name(node)} → ingenting`);
        continue;
      }
      if (hit === node || node.contains(hit)) continue;
      out.push(`${name(node)} → ${name(hit)}`);
    }
    return out;
  });
  expect(stolen).toEqual([]);
}

/** Påstanden om ÉN skjerm: bryterne, utfoldingsknappene, ghost-knappene, og
 *  ingen tyveri. */
async function assertHitTargets(
  page: Page,
  where: string,
  expect_: { toggles: number; expands: number; ghosts: number },
): Promise<void> {
  const toggles = await measure(page, 'button[role="switch"]');
  const expands = await measure(page, '[data-testid$="-expand"]');
  const ghosts = await measure(page, 'button[data-variant="ghost"]');

  // At det faktisk STO noe der å måle. Uten dette ville et skjermbytte som
  // gikk galt gitt en grønn test om en tom skjerm.
  expect(toggles.length, `${where}: antall brytere målt`).toBe(expect_.toggles);
  expect(expands.length, `${where}: antall utfoldingsknapper målt`).toBe(
    expect_.expands,
  );
  expect(ghosts.length, `${where}: antall ghost-knapper målt`).toBe(
    expect_.ghosts,
  );

  for (const t of toggles) {
    expect(t.hitH, `${where}/${t.id}: treffhøyde`).toBeGreaterThanOrEqual(
      TOGGLE_FLOOR - PROBE_STEP,
    );
    expect(t.hitW, `${where}/${t.id}: treffbredde`).toBeGreaterThanOrEqual(
      TOGGLE_FLOOR - PROBE_STEP,
    );
    // …og bryteren ser fortsatt ut som en bryter. Det er halve kontrakten.
    expect(t.boxH, `${where}/${t.id}: boksen står stille`).toBe(22);
    expect(t.boxW, `${where}/${t.id}: boksen står stille`).toBe(40);
  }

  for (const e of expands) {
    expect(e.hitH, `${where}/${e.id}: treffhøyde`).toBeGreaterThanOrEqual(
      EXPAND_FLOOR - PROBE_STEP,
    );
    // Sidelengs skal den IKKE ha vokst — knappene i denne appen står 8 px fra
    // hverandre der det er trangest, og en flate som stjeler er verre enn en
    // liten knapp. Boksen er bredere enn 44, så det er ikke et gulv her.
    expect(e.hitW, `${where}/${e.id}: ingen sidelengs vekst`).toBeCloseTo(
      e.boxW,
      0,
    );
    // Raden er 44 px høy og skal LESES som én linje: knappen selv må bli
    // stående på ~30.
    expect(e.boxH, `${where}/${e.id}: boksen står stille`).toBeLessThan(34);
  }

  for (const g of ghosts) {
    expect(g.hitH, `${where}/${g.id}: treffhøyde`).toBeGreaterThanOrEqual(
      GHOST_FLOOR - PROBE_STEP,
    );
  }

  await nobodyStolen(page);
}

// ── Skjermene ───────────────────────────────────────────────────────────────

const X32 = {
  id: "x32",
  name: "Behringer X32",
  backend: "coreaudio",
  inputChannels: 2,
  sampleRates: [48000],
  isDefault: true,
};

const FIXTURES: Fixtures = { ...BOOT_FIXTURES, list_audio_devices: [X32] };

const CHOSEN = {
  ...SETTLED_SETTINGS,
  deviceId: "x32",
  deviceName: "Behringer X32",
  saveFolder: "/Users/frivillig/SundayRec",
};

test.describe("treffflater", () => {
  test("kontrollrommet: bryterne og utfoldingsknappene, lukket", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });
    await expect(page.getByTestId("control-folder-expand")).toBeVisible();
    // Kamera og auto-opptak er de to kortene som har en bryter i lead-en, og
    // de står ved siden av hverandre i stabelen — 6 px fra hverandre. Det er
    // nettopp der en for grådig flate ville tatt naboens bryter.
    await assertHitTargets(page, "record", {
      toggles: 2,
      expands: 3,
      ghosts: 1,
    });
  });

  test("kontrollrommet: med et kort foldet ut", async ({ page }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });
    // Utfoldet er det verste tilfellet for utfoldingsknappen: kroppen kommer
    // rett under den, og flaten stikker 7 px ned i den.
    // (`::after` er −8 px fra padding-boksen; kanten spiser 1 av dem.)
    await page.getByTestId("control-notify-expand").click();
    await expect(page.getByTestId("setup-notify")).toBeVisible();
    // Tre og ikke fire: kroppen har to brytere, men e-postbryteren står bak en
    // `Gate` som er `inert` uten `smtp`-featuren. Den skal IKKE svare på et
    // trykk, og telles derfor ikke — se `measure`.
    // To ghost-knapper: kilde-kortets «Endre» OG kroppens «Test»-knapp
    // (`notify-test`).
    await assertHitTargets(page, "record/notify", {
      toggles: 3,
      expands: 3,
      ghosts: 2,
    });
  });

  test("kontrollrommet: kvalitet og mappe foldet ut samtidig", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });
    await page.getByTestId("control-folder-expand").click();
    await page.getByTestId("control-quality-expand").click();
    await expect(page.getByTestId("setup-quality")).toBeVisible();
    await assertHitTargets(page, "record/folder+quality", {
      toggles: 2,
      expands: 3,
      ghosts: 1,
    });
  });

  test("Innstillinger/Avansert: hele bryterrekka", async ({ page }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: CHOSEN,
      goto: "settings",
    });
    await expect(page.getByTestId("adv-silence-control-input")).toBeVisible();
    // Den lengste siden i appen, og den tetteste: her står brytere 43 px fra
    // hverandre og 8 px fra en slett-knapp sidelengs.
    const toggles = await measure(page, 'button[role="switch"]');
    expect(toggles.length).toBeGreaterThanOrEqual(6);
    for (const t of toggles) {
      expect(t.hitH, `advanced/${t.id}: treffhøyde`).toBeGreaterThanOrEqual(
        TOGGLE_FLOOR - PROBE_STEP,
      );
      expect(t.boxH, `advanced/${t.id}: boksen står stille`).toBe(22);
    }
    // F1-UX1/W2: den TETTESTE siden i appen er også den med flest ghost-
    // knapper (kopier, forhåndsvis, eksporter/importer, diagnose, …) — samme
    // gulv, og `toBeGreaterThanOrEqual` av samme grunn som toggle-tallet over:
    // denne siden vokser oftest.
    const ghosts = await measure(page, 'button[data-variant="ghost"]');
    expect(ghosts.length).toBeGreaterThanOrEqual(9);
    for (const g of ghosts) {
      expect(g.hitH, `advanced/${g.id}: treffhøyde`).toBeGreaterThanOrEqual(
        GHOST_FLOOR - PROBE_STEP,
      );
    }
    await nobodyStolen(page);
  });

  test("editoren: bryterne i lyd-steget", async ({ page }) => {
    await boot(page, {
      fixtures: editorFixtures(),
      settings: SETTLED_SETTINGS,
      goto: "editor",
    });
    await page.evaluate(
      (f) =>
        (
          window as unknown as { openEditorWithFile: (p: string) => void }
        ).openEditorWithFile(f),
      FILE,
    );
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "ready",
    );
    await page.getByTestId("editor-steps-row-sound").click();
    await expect(page.getByTestId("editor-auto-toggle")).toBeVisible();

    const toggles = await measure(page, 'button[role="switch"]');
    expect(toggles.map((t) => t.id)).toContain("editor-auto-toggle");
    for (const t of toggles) {
      expect(t.hitH, `editor/${t.id}: treffhøyde`).toBeGreaterThanOrEqual(
        TOGGLE_FLOOR - PROBE_STEP,
      );
      expect(t.boxH, `editor/${t.id}: boksen står stille`).toBe(22);
    }
    // F1-UX1/W2: to — «Lukk» øverst og mikserens «Åpne miksepanelet».
    const ghosts = await measure(page, 'button[data-variant="ghost"]');
    expect(ghosts.length).toBe(2);
    for (const g of ghosts) {
      expect(g.hitH, `editor/${g.id}: treffhøyde`).toBeGreaterThanOrEqual(
        GHOST_FLOOR - PROBE_STEP,
      );
    }
    await nobodyStolen(page);
  });

  // F1-UX1/W2: bibliotekets rader — tre ghost-knapper per rad-nabolag
  // («Slett» på hver rad, «Endre» ved autoslett-linja, «Papirkurv»-lenken) i
  // en liste som er den ene skjermen en frivillig scroller MEST i, på jakt
  // etter ett bestemt opptak.
  test("Bibliotek: ghost-knappene på radene", async ({ page }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        recordings_list: [
          recordingRow({ file_path: "/Users/frivillig/SundayRec/a.mp3" }),
          recordingRow({ file_path: "/Users/frivillig/SundayRec/b.mp3" }),
        ],
      },
      settings: CHOSEN,
      goto: "search",
    });
    await expect(page.getByTestId("library-row")).toHaveCount(2);

    const ghosts = await measure(page, 'button[data-variant="ghost"]');
    // To rader × «Slett» + «Endre» (autoslett) + «Papirkurv».
    expect(ghosts.length).toBe(4);
    for (const g of ghosts) {
      expect(g.hitH, `library/${g.id}: treffhøyde`).toBeGreaterThanOrEqual(
        GHOST_FLOOR - PROBE_STEP,
      );
    }
    await nobodyStolen(page);
  });
});
