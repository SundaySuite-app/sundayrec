import { describe, expect, it } from "vitest";

import type { RelaySubscriptionStatus } from "@legacy/bindings/RelaySubscriptionStatus";

import {
  RELAY_RESEND_COOLDOWN_MS,
  relayTransport,
  relayView,
  resendWaitMinutes,
  type RelayViewInput,
} from "./relay-core";

const NOW = 1_756_000_000_000;

function status(
  over: Partial<RelaySubscriptionStatus> = {},
): RelaySubscriptionStatus {
  return {
    endpointBuilt: true,
    state: null,
    address: null,
    enrolledAt: null,
    confirmedAt: null,
    queued: 0,
    ...over,
  };
}

function view(over: Partial<RelayViewInput> = {}) {
  return relayView({
    facts: status(),
    savedAddress: "",
    dirty: false,
    lastResendAt: null,
    now: NOW,
    ...over,
  });
}

const ENROLLED = {
  facts: status({ state: "pending", address: "lyd@brynmenighet.no" }),
  savedAddress: "lyd@brynmenighet.no",
} satisfies Partial<RelayViewInput>;

describe("ingenting påmeldt", () => {
  it("fabrikkfersk maskin ⇒ «Bekreft», og den er av til adressen er lagret", () => {
    const v = view();
    expect(v.step).toBe("none");
    expect(v.showConfirm).toBe(true);
    expect(v.confirmBlock).toBe("unsaved");
    expect(v.showResend).toBe(false);
    expect(v.showUnsubscribe).toBe(false);
    expect(v.transport).toBe(false);
  });

  it("en lagret adresse åpner knappen", () => {
    expect(
      view({ savedAddress: "lyd@brynmenighet.no" }).confirmBlock,
    ).toBeNull();
  });

  it("et uslagret utkast lukker den igjen", () => {
    // Å melde på en adresse brukeren ikke har trykket Lagre på er å melde på
    // noe hun ikke har sagt seg ferdig med — og bekreftelsesmailen går til den
    // LAGREDE adressen uansett, så knappen ville gjort noe annet enn den viser.
    expect(
      view({ savedAddress: "lyd@brynmenighet.no", dirty: true }).confirmBlock,
    ).toBe("unsaved");
  });

  it("fakta som ikke er lest ennå leses som «ingenting påmeldt», ikke som en sperre", () => {
    // Halvsekundet før bakenden svarer skal ikke være en grå knapp: gjetningen
    // er ufarlig, for handlingen den tilbyr er nøyaktig den brukeren ba om.
    const v = view({ facts: null, savedAddress: "lyd@brynmenighet.no" });
    expect(v.step).toBe("none");
    expect(v.confirmBlock).toBeNull();
  });
});

describe("venter på bekreftelse", () => {
  it("samme adresse ⇒ «Send på nytt», ikke «Bekreft» en gang til", () => {
    const v = view(ENROLLED);
    expect(v.step).toBe("pending");
    expect(v.showResend).toBe(true);
    expect(v.showConfirm).toBe(false);
    expect(v.resendWaitMs).toBe(0);
  });

  it("en NY lagret adresse tilbyr «Bekreft» igjen", () => {
    // `relay_subscribe` erstatter raden med vilje — det er den ærlige lesningen
    // av «brukeren skrev en annen adresse». Uten denne raden ville en skrivefeil
    // låst maskinen i «venter» til noen fant på å slette abonnementet.
    const v = view({ ...ENROLLED, savedAddress: "post@brynmenighet.no" });
    expect(v.showConfirm).toBe(true);
    expect(v.confirmBlock).toBeNull();
    expect(v.showResend).toBe(false);
  });

  it("store og små bokstaver er samme adresse", () => {
    // Bakenden folder (`normalize_address`); en sammenligning som ikke gjør det
    // ville tilbudt en ny påmelding for ingenting.
    const v = view({ ...ENROLLED, savedAddress: "  Lyd@Brynmenighet.NO " });
    expect(v.showResend).toBe(true);
    expect(v.showConfirm).toBe(false);
  });

  it("sperren teller ned og slipper opp av seg selv", () => {
    const justSent = view({ ...ENROLLED, lastResendAt: NOW });
    expect(justSent.resendWaitMs).toBe(RELAY_RESEND_COOLDOWN_MS);
    const halfway = view({
      ...ENROLLED,
      lastResendAt: NOW - RELAY_RESEND_COOLDOWN_MS / 2,
    });
    expect(halfway.resendWaitMs).toBe(RELAY_RESEND_COOLDOWN_MS / 2);
    const done = view({
      ...ENROLLED,
      lastResendAt: NOW - RELAY_RESEND_COOLDOWN_MS,
    });
    expect(done.resendWaitMs).toBe(0);
  });

  it("sperren er en hel klokke, aldri negativ", () => {
    expect(
      view({ ...ENROLLED, lastResendAt: NOW - RELAY_RESEND_COOLDOWN_MS * 3 })
        .resendWaitMs,
    ).toBe(0);
  });
});

describe("bekreftet", () => {
  const confirmed = {
    facts: status({
      state: "confirmed",
      address: "lyd@brynmenighet.no",
      confirmedAt: NOW - 1000,
    }),
    savedAddress: "lyd@brynmenighet.no",
  } satisfies Partial<RelayViewInput>;

  it("ingen «Bekreft» igjen — bare «Meld meg av»", () => {
    const v = view(confirmed);
    expect(v.step).toBe("confirmed");
    expect(v.showConfirm).toBe(false);
    expect(v.showResend).toBe(false);
    expect(v.showUnsubscribe).toBe(true);
    expect(v.transport).toBe(true);
    expect(v.address).toBe("lyd@brynmenighet.no");
  });

  it("en annen adresse i feltet endrer ikke at DENNE er bekreftet", () => {
    // Tilstanden beskriver abonnementet, ikke utkastet. Å vise «Bekreft» her
    // ville tilbudt å bytte bort en fungerende sendevei fordi noen begynte å
    // skrive i et felt.
    const v = view({ ...confirmed, savedAddress: "post@brynmenighet.no" });
    expect(v.showConfirm).toBe(false);
    expect(v.address).toBe("lyd@brynmenighet.no");
  });
});

describe("adressen tar ikke imot e-post", () => {
  const suppressed = {
    facts: status({ state: "suppressed", address: "lyd@brynmenighet.no" }),
    savedAddress: "lyd@brynmenighet.no",
  } satisfies Partial<RelayViewInput>;

  it("samme adresse på nytt er ikke et forslag — den sier hvorfor", () => {
    const v = view(suppressed);
    expect(v.step).toBe("suppressed");
    expect(v.showConfirm).toBe(true);
    expect(v.confirmBlock).toBe("sameSuppressed");
    expect(v.showUnsubscribe).toBe(true);
    expect(v.transport).toBe(false);
  });

  it("en annen lagret adresse åpner «Bekreft»", () => {
    expect(
      view({ ...suppressed, savedAddress: "post@brynmenighet.no" })
        .confirmBlock,
    ).toBeNull();
  });
});

describe("en build uten endepunkt", () => {
  it("sier «ikke tilgjengelig» — også når den husker et bekreftet abonnement", () => {
    // Nedgradering: raden overlever, sendeveien gjør ikke. Å si «bekreftet» her
    // ville vært et grønt kort om e-post som ikke kan gå noe sted.
    const v = view({
      facts: status({
        endpointBuilt: false,
        state: "confirmed",
        address: "lyd@brynmenighet.no",
      }),
      savedAddress: "lyd@brynmenighet.no",
    });
    expect(v.step).toBe("unavailable");
    expect(v.transport).toBe(false);
    expect(v.confirmBlock).toBe("noEndpoint");
    expect(v.showUnsubscribe).toBe(false);
  });
});

describe("utboksen", () => {
  it("noe som venter på nett er verdt å si", () => {
    expect(view({ facts: status({ queued: 2 }) }).queued).toBe(true);
    expect(view({ facts: status({ queued: 0 }) }).queued).toBe(false);
    expect(view({ facts: null }).queued).toBe(false);
  });
});

describe("relayTransport", () => {
  it("er det samme svaret som tabellens `transport`, rad for rad", () => {
    // To innganger til det samme spørsmålet — kontrollrommet spør den korte,
    // siden spør tabellen — og skjøten mellom dem er nøyaktig der en uenighet
    // ville stått uoppdaget: et grønt kort 5 ved siden av en side som sier
    // «ikke bekreftet».
    const rows: RelaySubscriptionStatus[] = [
      status(),
      status({ state: "pending", address: "a@b.no" }),
      status({ state: "confirmed", address: "a@b.no" }),
      status({ state: "suppressed", address: "a@b.no" }),
      status({ endpointBuilt: false, state: "confirmed", address: "a@b.no" }),
      status({ endpointBuilt: false }),
    ];
    for (const facts of rows) {
      expect(relayTransport(facts), JSON.stringify(facts)).toBe(
        view({ facts }).transport,
      );
    }
  });

  it("ulest er ulest — ikke «nei»", () => {
    expect(relayTransport(null)).toBeNull();
  });
});

describe("resendWaitMinutes", () => {
  it("runder opp, og sier aldri «om 0 min»", () => {
    expect(resendWaitMinutes(RELAY_RESEND_COOLDOWN_MS)).toBe(10);
    expect(resendWaitMinutes(60_000)).toBe(1);
    expect(resendWaitMinutes(61_000)).toBe(2);
    expect(resendWaitMinutes(1)).toBe(1);
  });
});
