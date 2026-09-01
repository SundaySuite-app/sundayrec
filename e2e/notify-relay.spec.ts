import type { Page } from "@playwright/test";
import { expect, test } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  relayEmpty,
  SETTLED_SETTINGS,
  VOID,
} from "./harness";

// E-postreléet, hele tilstandsmaskinen (A5).
//
// Dobbel opt-in har fire tilstander, og ingen av dem er den samme skjermen:
// ingenting påmeldt, venter på at noen trykker på en lenke i en innboks,
// bekreftet, og «adressen tar ikke imot e-post». `relay-core.test.ts` pinner
// TABELLEN; denne fila pinner at skjermen faktisk følger den — inkludert
// tingen ingen enhetstest kan se: at bekreftelsen skjer UTENFOR appen, og at
// siden derfor må lese statusen på nytt for å oppdage den.
//
// ## Bakenden er en localStorage-rad
//
// Fem kommandoer over én tilstand som må overleve en `page.reload()`, fordi
// reload er nettopp hvordan denne suiten simulerer «noen trykket på lenken i
// e-posten sin»: raden endres utenfra, og siden må hente den ferskt. En
// variabel på `window` ville blitt nullstilt av samme reload og gjort hver
// slik overgang til en test av seeden.

/** Der den falske abonnementsraden bor mellom lesningene. */
const RELAY_DB_KEY = "__e2e.relayDb";

/** Fixtures for de fem kommandoene, over den ene lagrede raden.
 *
 *  Merk hva `relay_unsubscribe` IKKE gjør: den sletter ikke raden. Bakenden
 *  gjør heller ikke det (`commands/notify_relay.rs` — pumpen er gatet på at
 *  raden finnes, så en sletting her ville strandet nettopp den forespørselen
 *  klikket køet). Skjermen skal fortsette å vise et abonnement til
 *  avmeldingen har gått ut, og det er den ærlige oppførselen å teste. */
function relayFixtures(initial: Record<string, unknown>) {
  const read = `JSON.parse(window.localStorage.getItem(${JSON.stringify(RELAY_DB_KEY)}) ?? "null")`;
  const write = (expr: string) =>
    `(() => { const next = ${expr}; window.localStorage.setItem(${JSON.stringify(RELAY_DB_KEY)}, JSON.stringify(next)); return next })()`;
  return {
    relay_status: fn(`() => ${read} ?? ${write(JSON.stringify(initial))}`),
    relay_subscribe: fn(
      `(args) => ${write(`{ ...(${read} ?? {}), state: "pending", address: String(args.address), enrolledAt: 1, confirmedAt: null, queued: 1 }`)}`,
    ),
    relay_resend: fn(
      `() => ${write(`{ ...(${read} ?? {}), queued: (${read}?.queued ?? 0) + 1 }`)}`,
    ),
    relay_unsubscribe: fn(
      `() => ${write(`{ ...(${read} ?? {}), queued: (${read}?.queued ?? 0) + 1 }`)}`,
    ),
    relay_send_test: VOID,
  };
}

/** Boot on question 5 with the relay stubbed. */
async function bootNotify(
  page: Page,
  opts: {
    relay?: Record<string, unknown>;
    settings?: Record<string, unknown>;
    fixtures?: Record<string, unknown>;
  } = {},
): Promise<void> {
  const initial = opts.relay ?? relayEmpty();
  await page.addInitScript(
    ([key, value]) => {
      if (!window.localStorage.getItem(key as string)) {
        window.localStorage.setItem(key as string, JSON.stringify(value));
      }
    },
    [RELAY_DB_KEY, initial] as const,
  );
  await boot(page, {
    fixtures: {
      ...BOOT_FIXTURES,
      ...relayFixtures(initial),
      ...(opts.fixtures ?? {}),
    },
    settings: { ...SETTLED_SETTINGS, ...(opts.settings ?? {}) },
    goto: "settings:sharing",
  });
  await expect(page.getByTestId("setup-notify")).toBeVisible();
}

/** «Noen åpnet e-posten og trykket på lenken» — en endring som skjer utenfor
 *  appen, og som siden bare oppdager ved å spørre på nytt. */
async function fromTheInbox(
  page: Page,
  patch: Record<string, unknown>,
): Promise<void> {
  await page.evaluate(
    ([key, value]) => {
      const k = key as string;
      const current = JSON.parse(
        window.localStorage.getItem(k) ?? "{}",
      ) as Record<string, unknown>;
      window.localStorage.setItem(
        k,
        JSON.stringify({ ...current, ...(value as Record<string, unknown>) }),
      );
    },
    [RELAY_DB_KEY, patch] as const,
  );
  await page.reload();
  await expect(page.getByTestId("setup-notify")).toBeVisible();
}

const ADDRESS = "lyd@brynmenighet.no";

test.describe("reléet er hovedveien", () => {
  test("hele veien: ikke bekreftet → venter → bekreftet → meldt av", async ({
    page,
  }) => {
    await bootNotify(page, { settings: { emailAddress: ADDRESS } });

    // ── 1. Ingenting påmeldt ────────────────────────────────────────────────
    const state = page.getByTestId("notify-relay-state");
    await expect(state).toHaveText("Ikke bekreftet");
    await expect(page.getByTestId("notify-relay-resend")).toHaveCount(0);
    await expect(page.getByTestId("notify-relay-unsubscribe")).toHaveCount(0);
    // Kvitteringsbryteren finnes ikke uten et bekreftet abonnement: den ville
    // vært en av-og-på for noe som ikke kan skje.
    await expect(page.getByTestId("notify-receipt")).toHaveCount(0);
    // Ingen SMTP og ingen bekreftelse ⇒ porten er stengt, men på den måten
    // som er noe å GJØRE noe med.
    await expect(page.getByTestId("notify-email-gate")).toHaveAttribute(
      "data-gate",
      "unconfigured",
    );
    await expect(page.getByTestId("notify-test")).toHaveAttribute(
      "aria-disabled",
      "true",
    );

    // ── 2. «Bekreft e-postadressen» ─────────────────────────────────────────
    await page.getByTestId("notify-relay-confirm").click();
    await expect(state).toHaveText("Venter på bekreftelse");
    // Adressen står i teksten: en «sjekk innboksen» uten å si HVILKEN innboks
    // er en beskjed man ikke kan handle på.
    await expect(page.getByTestId("notify-relay")).toContainText(ADDRESS);
    await expect(page.getByTestId("toast-host")).toContainText(ADDRESS);
    // Ingen ny «Bekreft» — den samme adressen er allerede på vei.
    await expect(page.getByTestId("notify-relay-confirm")).toHaveCount(0);
    // …og fortsatt ingen sendevei: dobbel opt-in betyr at ingenting går ut før
    // noen har trykket i innboksen.
    await expect(page.getByTestId("notify-email-gate")).toHaveAttribute(
      "data-gate",
      "unconfigured",
    );

    // ── 3. «Send på nytt», og sperren som følger ────────────────────────────
    const resend = page.getByTestId("notify-relay-resend");
    await expect(resend).not.toHaveAttribute("aria-disabled", "true");
    await resend.click();
    await expect(resend).toHaveAttribute("aria-disabled", "true");
    // Grunnen står på knappen, ikke i en tom grå flate.
    await expect(resend).toHaveAttribute("title", /Vent \d+ min/);

    // ── 4. Noen trykker på lenken i innboksen ───────────────────────────────
    await fromTheInbox(page, { state: "confirmed", confirmedAt: 2, queued: 0 });
    await expect(state).toHaveText("Bekreftet");
    await expect(page.getByTestId("notify-email-gate")).toHaveAttribute(
      "data-gate",
      "ok",
    );
    await expect(page.getByTestId("notify-email-gate-banner")).toHaveCount(0);
    // Kvitteringsbryteren dukker opp, og sier hva den gjelder.
    await expect(
      page.getByTestId("notify-receipt-control-input"),
    ).toBeVisible();
    await expect(page.getByTestId("notify-receipt")).toContainText(
      "Bare for planlagte opptak",
    );
    // «Send en test» er åpen — uten at noen har rørt en SMTP-innstilling.
    await expect(page.getByTestId("notify-test")).not.toHaveAttribute(
      "aria-disabled",
      "true",
    );

    // ── 5. «Meld meg av» ────────────────────────────────────────────────────
    await page.getByTestId("notify-relay-unsubscribe").click();
    // Skjermen sier fortsatt «Bekreftet», og det er ikke en feil: maskinen ER
    // påmeldt til avmeldingen har gått ut. Køen er det som er nytt.
    await expect(state).toHaveText("Bekreftet");
    await expect(page.getByTestId("notify-relay")).toContainText("kø");

    // …og når pumpen har fått den ut, er raden borte.
    await fromTheInbox(page, { state: null, address: null, queued: 0 });
    await expect(state).toHaveText("Ikke bekreftet");
    await expect(page.getByTestId("notify-receipt")).toHaveCount(0);
  });

  test("«Bekreft» er av til adressen er LAGRET", async ({ page }) => {
    // En påmelding bruker en ekte e-post og én av endepunktets tre
    // bekreftelser per døgn, så den skal ikke gå på et halvskrevet utkast.
    await bootNotify(page);
    const confirm = page.getByTestId("notify-relay-confirm");
    await expect(confirm).toHaveAttribute("aria-disabled", "true");
    await expect(confirm).toHaveAttribute("title", /trykk Lagre først/);

    await page.getByTestId("notify-address-control-input").fill(ADDRESS);
    // Skrevet, men ikke lagret — fortsatt av.
    await expect(confirm).toHaveAttribute("aria-disabled", "true");

    await page.getByTestId("notify-save").click();
    await expect(page.getByTestId("notify-address-receipt")).toHaveText(
      "Lagret ✓",
    );
    await expect(confirm).not.toHaveAttribute("aria-disabled", "true");
  });

  test("en adresse som avviser e-post sier det, og tilbyr en vei ut", async ({
    page,
  }) => {
    // 410 `recipient_suppressed` fra endepunktet blir en LOKAL tilstand, og
    // det er hele poenget: uten den ville en frivillig bare fått stillhet.
    await bootNotify(page, {
      settings: { emailAddress: ADDRESS },
      relay: relayEmpty({ state: "suppressed", address: ADDRESS }),
    });

    await expect(page.getByTestId("notify-relay-state")).toHaveText(
      "Adressen tar ikke imot e-post",
    );
    await expect(page.getByTestId("notify-relay")).toContainText("i retur");
    // Ikke en sendevei — porten er stengt.
    await expect(page.getByTestId("notify-email-gate")).toHaveAttribute(
      "data-gate",
      "unconfigured",
    );
    // «Bekreft» på DEN SAMME adressen er ikke et forslag, og knappen sier det.
    const confirm = page.getByTestId("notify-relay-confirm");
    await expect(confirm).toHaveAttribute("aria-disabled", "true");
    await expect(confirm).toHaveAttribute("title", /annen adresse/);
    // Veien ut nummer to: meld deg av.
    await expect(page.getByTestId("notify-relay-unsubscribe")).toBeVisible();

    // En annen adresse, lagret, åpner knappen igjen.
    await page
      .getByTestId("notify-address-control-input")
      .fill("post@brynmenighet.no");
    await page.getByTestId("notify-save").click();
    await expect(confirm).not.toHaveAttribute("aria-disabled", "true");
  });

  test("en build uten endepunkt lover ingenting", async ({ page }) => {
    // Uten `SUNDAYREC_NOTIFY_URL` i byggesteget finnes det ingen tjeneste å
    // melde seg på. En knapp som køet rader ingenting kommer til å sende er
    // verre enn ingen knapp.
    await bootNotify(page, {
      settings: { emailAddress: ADDRESS },
      relay: relayEmpty({ endpointBuilt: false }),
      fixtures: { email_status: { featureBuilt: true } },
    });

    await expect(page.getByTestId("notify-relay-state")).toHaveText(
      "Ikke tilgjengelig",
    );
    const confirm = page.getByTestId("notify-relay-confirm");
    await expect(confirm).toHaveAttribute("aria-disabled", "true");
    await expect(confirm).toHaveAttribute("title", /SundaySuite/);
    // Med e-postfeaturen i bygget finnes SMTP-veien fortsatt, så gaten sier
    // «ikke satt opp» og ikke «finnes ikke».
    await expect(page.getByTestId("notify-email-gate")).toHaveAttribute(
      "data-gate",
      "unconfigured",
    );
  });

  test("verken relé eller e-postfeature ⇒ «ikke tilgjengelig», ikke «sett det opp»", async ({
    page,
  }) => {
    await bootNotify(page, {
      settings: { emailAddress: ADDRESS },
      relay: relayEmpty({ endpointBuilt: false }),
      fixtures: { email_status: { featureBuilt: false } },
    });
    await expect(page.getByTestId("notify-email-gate")).toHaveAttribute(
      "data-gate",
      "unavailable",
    );
    await expect(page.getByTestId("notify-email-gate-banner")).toContainText(
      "ikke bygget inn i denne versjonen",
    );
  });
});

test.describe("«Send en test» går gjennom den AKTIVE kanalen", () => {
  test("bekreftet relé ⇒ reléet, ikke SMTP", async ({ page }) => {
    // SMTP-kommandoen KASTER her. Går testen grønt, ble den aldri kalt — det
    // er den eneste måten å bevise hvilken av de to kanalene knappen brukte.
    await bootNotify(page, {
      settings: { emailAddress: ADDRESS },
      relay: relayEmpty({ state: "confirmed", address: ADDRESS }),
      fixtures: {
        email_send_test: fn(
          "() => { throw new Error('the SMTP path must not be used') }",
        ),
      },
    });

    await page.getByTestId("notify-test").click();
    await expect(page.getByTestId("toast-host")).toContainText(
      `Test-e-post sendt til ${ADDRESS}`,
    );
  });

  test("uten bekreftelse, men med SMTP ⇒ SMTP-stien, uendret", async ({
    page,
  }) => {
    // Garantien til menighetene som allerede HAR en server: ingenting av dette
    // flyttet på dem.
    await bootNotify(page, {
      settings: {
        emailAddress: ADDRESS,
        emailSmtp: "smtp.kirken.no",
        emailSmtpUser: "opptak@kirken.no",
      },
      fixtures: {
        email_status: { featureBuilt: true },
        email_has_smtp_password: true,
        relay_send_test: fn(
          "() => { throw new Error('the relay must not be used') }",
        ),
        email_send_test: VOID,
      },
    });

    await expect(page.getByTestId("notify-email-gate")).toHaveAttribute(
      "data-gate",
      "ok",
    );
    await page.getByTestId("notify-test").click();
    await expect(page.getByTestId("toast-host")).toContainText(
      `Test-e-post sendt til ${ADDRESS}`,
    );
  });
});
