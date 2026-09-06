/**
 * Hva som skjer når en innstilling endres — hele sekvensen, uten DOM og uten
 * hooks.
 *
 * ## Én lagringsmodell
 *
 * `bind-setting-core.ts` (som denne modulen IMPORTERER fra, ikke kopierer)
 * avgjør NÅR en endring committes og HVILKEN verdi som committes. Denne
 * modulen avgjør hva som skjer etterpå, og rekkefølgen er hele poenget:
 *
 *     validate → guard/confirmIf → apply → persist → kvittering | revert
 *
 * Rekkefølgen er ikke smakssak. `validate` før `guard`, ellers spør vi «vil du
 * virkelig?» om en verdi vi uansett skal avvise. `apply` før `persist`, ellers
 * ser brukeren ingenting skje mens skrivningen går. Og `revert` ETTER en
 * feilet `persist`, som er den ene tingen som er strengere her enn i legacy.
 *
 * ## Revert-på-feil
 *
 * `legacy/renderer/ui/bind-setting.ts` lar verdien STÅ når `settings_save`
 * feiler: den viser en feil-toast og går videre. Skjermen påstår da at
 * innstillingen er én ting og basen sier en annen — og neste gang appen
 * starter «forsvant» endringen. En frivillig som ser det har ingen måte å
 * vite hvilken av de to som gjelder.
 *
 * Her rulles verdien tilbake til det som faktisk står lagret, og toasten sier
 * hva som gikk galt. Skjermen og basen er enige hele tiden, også når det
 * mislykkes. Det er den ene bevisste avvikelsen fra legacy-oppførselen.
 *
 * ## Hvorfor alt er injisert
 *
 * Dialog, toast, skrivning og oversettelse kommer inn som funksjoner. Ikke for
 * abstraksjonens skyld — for at HELE sekvensen skal kunne kjøres i node-gaten,
 * tabell for tabell, inkludert de veiene som bare oppstår når noe feiler. Det
 * er nettopp de veiene ingen tester manuelt.
 */

import { isRealChange, type SettingValue } from "@lib/ui/bind-setting-core";

/**
 * Verdien en kontroll leverer, smalnet til den TYPEN som allerede står lagret.
 *
 * ## Hvorfor dette er nødvendig
 *
 * Et `<select>` leverer alltid en STRENG. Halvparten av innstillingene bak en
 * select er tall i Rust (`preRollSeconds`, `splitMinutes`, `manualMaxMinutes`,
 * `silenceTimeoutMinutes`, `reminderMinutes`), og `Settings` er en ts-rs-type
 * som deserialiseres strengt: `"30"` der serde venter `i32` avviser HELE
 * lagringen, ikke bare det ene feltet. Skjermen ville sagt «Lagret ✓» på en
 * skrivning som aldri landet, og alt annet brukeren endret i samme byge ville
 * fulgt med i fallet.
 *
 * P1a løste det ett kallsted om gangen (`reminder.set(Number(next))`), som er
 * nøyaktig formen på en skjøtefeil: to steder som må huske det samme.
 * `previous` — den lagrede verdien — er fasiten, og den er alltid tilgjengelig.
 *
 * `bitrate` er strengen `"256"` i Rust og skal BLI en streng; det er derfor
 * typen til den lagrede verdien og ikke «ser det ut som et tall?» som avgjør.
 */
export function narrowToStored(
  previous: SettingValue,
  next: SettingValue,
): SettingValue {
  if (typeof previous !== "number" || typeof next !== "string") return next;
  const trimmed = next.trim();
  if (trimmed === "") return next;
  const parsed = Number(trimmed);
  // Ikke et tall ⇒ la den stå som den er, så `validate` kan avvise den med en
  // setning i stedet for at en `NaN` sniker seg inn i basen.
  return Number.isFinite(parsed) ? parsed : next;
}

/** Kvitteringen brukeren ser ved siden av kontrollen. */
export type Receipt = "idle" | "saving" | "saved" | "failed";

/** Det en vakt vil spørre om før endringen anvendes. */
export interface GuardDescriptor {
  title: string;
  message?: string;
  confirmLabel?: string;
  cancelLabel?: string;
}

/** Hvordan det gikk. Én verdi per vei gjennom sekvensen. */
export type CommitOutcome =
  /** Ingen reell endring — ingen skrivning, ingen kvittering, ingen støy. */
  | "skipped"
  /** Avvist av `validate`. Verdien står, feilmeldingen vises, ingenting skrives. */
  | "invalid"
  /** Vakten spurte og brukeren sa nei. Rullet tilbake. */
  | "declined"
  | "saved"
  /** Skrivningen feilet. Rullet tilbake + toast. */
  | "failed";

export interface CommitResult {
  outcome: CommitOutcome;
  /** Meldingen fra `validate`, ellers `null`. */
  error: string | null;
}

export interface CommitDeps<T> {
  /** Sist committede verdi. */
  previous: SettingValue;
  /** Verdien kontrollen står på nå. */
  next: SettingValue;
  /** Smalne råverdien før validering/anvendelse. */
  coerce?: (raw: SettingValue) => T;
  /** Returner en melding for å avvise verdien. */
  validate?: (value: T) => string | null;
  /** Returner en beskrivelse for å spørre først. */
  confirmIf?: (value: T) => GuardDescriptor | null;
  /** Still spørsmålet. `app/ui/dialog.ts` som standard. */
  confirm: (guard: GuardDescriptor) => Promise<boolean>;
  /** Skriv verdien inn i innstillingene (`patchSettings`). */
  apply: (value: T) => void;
  /** Persister. `saveSettingsDebounced` som standard. `false` = det landet ikke. */
  persist: () => Promise<boolean>;
  /** Sett verdien tilbake — etter avslått vakt og etter feilet skrivning. */
  revert: (previous: SettingValue) => void;
  /** Si fra at det gikk galt. */
  toast: (kind: "error", msg: string) => void;
  /** Teksten til den toasten (`t('general.saveFailed')`), som funksjon så den
   *  hentes på språket som gjelder i det øyeblikket den trengs. */
  saveFailedMessage: () => string;
  /** Kjøres etter en vellykket lagring. */
  after?: (value: T) => void;
  /** Kvitteringen endret seg. */
  onReceipt?: (receipt: Receipt) => void;
  /** Valideringsfeilen endret seg (`null` = tøm den). */
  onError?: (message: string | null) => void;
}

/**
 * Kjør én commit gjennom hele sekvensen.
 *
 * Ren i den forstand som teller: alt som rører verden går gjennom `deps`, så en
 * test ser nøyaktig hvilke effekter som skjedde og i hvilken rekkefølge.
 */
export async function runCommit<T>(deps: CommitDeps<T>): Promise<CommitResult> {
  const receipt = (r: Receipt): void => deps.onReceipt?.(r);

  // 1. Er dette i det hele tatt en endring? Et `change`-event fyrer på en
  //    radiogruppe når SAMME valg klikkes om igjen, og et tekstfelt fyrer det
  //    på blur uten at noe er redigert. Å skrive og blinke «Lagret ✓» for en
  //    ikke-endring lærer brukeren å ignorere kvitteringen.
  if (!isRealChange(deps.previous, deps.next)) {
    return { outcome: "skipped", error: null };
  }

  const value = (deps.coerce ? deps.coerce(deps.next) : deps.next) as T;

  // 2. Validering før vakt: ingen grunn til å spørre «vil du virkelig?» om en
  //    verdi vi uansett skal avvise.
  const message = deps.validate?.(value) ?? null;
  if (message) {
    deps.onError?.(message);
    // Verdien blir STÅENDE. Å rulle tilbake her ville slettet det brukeren
    // holdt på å skrive, som er den eneste måten å fikse feilen på.
    return { outcome: "invalid", error: message };
  }
  deps.onError?.(null);

  // 3. Vakten. Den spør bare når endringen faktisk kan koste noe (et opptak
  //    som går, eller ett som er nær) — ellers blir den bakgrunnsstøy.
  const guard = deps.confirmIf?.(value) ?? null;
  if (guard) {
    const ok = await deps.confirm(guard);
    if (!ok) {
      deps.revert(deps.previous);
      return { outcome: "declined", error: null };
    }
  }

  // 4. Anvend først, så skriv: brukeren skal se at det skjedde mens
  //    skrivningen går.
  deps.apply(value);
  receipt("saving");

  const persisted = await deps.persist();
  if (!persisted) {
    // Strengere enn legacy — se toppen av fila.
    deps.revert(deps.previous);
    deps.toast("error", deps.saveFailedMessage());
    receipt("failed");
    return { outcome: "failed", error: null };
  }

  receipt("saved");
  deps.after?.(value);
  return { outcome: "saved", error: null };
}

// ── Køen: ingen commit forsvinner stille ────────────────────────────────────
//
// `useSetting` kan bli bedt om å committe mens en commit allerede er i lufta:
// et tekstfelt med etterslep armer en timer, brukeren skriver videre, og et
// blur eller den neste timeren lander midt i skrivningen som går. Den gamle
// hooken RYDDET den ventende timeren og returnerte så tomhendt hvis den var
// opptatt — altså kastet den redigeringen brukeren nettopp gjorde, uten et ord.
// «90» ble stående i basen mens skjermen sa «900», og de to var uenige helt til
// noen lastet appen på nytt.
//
// Reparasjonen er en KØ og ikke bare en omstokking av rekkefølgen: å sjekke
// `busy` før `clearTimeout` ville latt den armede timeren overleve ÉN gang, men
// den neste landingen midt i en skrivning ville tapt den på nøyaktig samme måte.
// Køen lover noe sterkere og enklere å si: blir det bedt om en commit mens en
// går, kjøres den om igjen når den første er ferdig — og da leses utkastet på
// nytt, så det er alltid SISTE verdi som lander og kvitteres for.
//
// Én plass i køen er nok. Tre redigeringer under samme skrivning er ikke tre
// verdier som skal skrives etter hverandre; det er én verdi (den siste) som
// skal skrives én gang.

/** Om en commit går, og om en til er ønsket når den er ferdig. */
export interface CommitQueue {
  /** En commit er i lufta. */
  busy: boolean;
  /** Det kom en forespørsel til mens den gikk. */
  queued: boolean;
}

/** Ingenting går, ingenting venter. */
export const IDLE_COMMIT_QUEUE: CommitQueue = { busy: false, queued: false };

/** Det som kan skje med køen. */
export type CommitQueueEvent =
  /** Noen ba om en commit (en timer landet, et blur, en Enter). */
  | "request"
  /** Den commiten som gikk er ferdig. */
  | "settled";

export interface CommitQueueStep {
  next: CommitQueue;
  /** `run` = kjør en commit nå (les utkastet på nytt først). `queue` = en går
   *  allerede og tar denne med seg. `idle` = ingenting mer å gjøre. */
  action: "run" | "queue" | "idle";
}

/**
 * Køens hele avgjørelse, som en ren overgang.
 *
 * Rekkefølgen `request` → (`request`)* → `settled` → … er nettopp den slags som
 * ser opplagt ut i en `useCallback` og er feil i det ene tilfellet ingen
 * klikker seg fram til. Her er den en tabell.
 */
export function stepCommitQueue(
  state: CommitQueue,
  event: CommitQueueEvent,
): CommitQueueStep {
  if (event === "request") {
    return state.busy
      ? { next: { busy: true, queued: true }, action: "queue" }
      : { next: { busy: true, queued: false }, action: "run" };
  }
  // `settled`: en ventende forespørsel blir den neste kjøringen, og køen tømmes
  // — den nye kjøringen skal kunne legge noe i den igjen.
  return state.queued
    ? { next: { busy: true, queued: false }, action: "run" }
    : { next: IDLE_COMMIT_QUEUE, action: "idle" };
}

// ── Den samme sekvensen for en skrivning som rører FLERE nøkler ──────────────
//
// `runCommit` eier én nøkkel, én forrige verdi og én kvittering. Noen valg er
// ikke én nøkkel: enhetsvalget er `deviceId` + `deviceName` + `deviceChannels`,
// opptaksmotoren er `classicFfmpegAudio` + `classicDirectshow`, og OS-varselet
// er `notifyStart` + `notifyStop`. Tre skrivninger med tre kvitteringer ville
// gitt et vindu der basen holder halve valget.
//
// Før dette hadde HVER av dem sin egen håndlagde lagringsmodell — patch, lagre,
// kvittering, tilbakerulling, toast — skrevet litt forskjellig hver gang, og de
// stedene som glemte tilbakerullingen lot skjermen påstå noe basen ikke hadde.
// Så: samme sekvens, samme rekkefølge, ett sted.
//
//     vakt → anvend → skriv → kvittering | rull tilbake
//
// `validate` finnes ikke her: en patch har ingen ÉN verdi å validere, og en
// vilkårlig predikatkrok ville vært en invitasjon til å legge beslutninger som
// hører hjemme i en ren kjerne inn i en komponent.

export interface PatchCommitDeps {
  /** Er dette en reell endring? `false` = ingen skrivning, ingen kvittering,
   *  ingen støy — samme regel som `isRealChange` gir `runCommit`. */
  changed: boolean;
  /** Spør først. Returner `false` for å avlyse. Ingen vakt = ingen spørsmål. */
  confirm?: () => Promise<boolean>;
  /** Skriv verdiene inn i innstillingene. */
  apply: () => void;
  /** Persister. `false` = det landet ikke. */
  persist: () => Promise<boolean>;
  /** Sett alt tilbake — etter avslått vakt og etter feilet skrivning. */
  revert: () => void;
  toast: (kind: "error", msg: string) => void;
  saveFailedMessage: () => string;
  /** Kjøres etter en vellykket lagring. En feil her rører ikke kvitteringen:
   *  skrivningen LANDET, og det er det kvitteringen svarer på. */
  after?: () => void | Promise<void>;
  onReceipt?: (receipt: Receipt) => void;
}

/** Kjør én fler-nøkkel-skrivning gjennom hele sekvensen. */
export async function runPatchCommit(
  deps: PatchCommitDeps,
): Promise<CommitOutcome> {
  const receipt = (r: Receipt): void => deps.onReceipt?.(r);

  if (!deps.changed) return "skipped";

  if (deps.confirm) {
    const ok = await deps.confirm();
    if (!ok) {
      // Ingen `apply` har skjedd ennå, men kallstedet kan ha satt en lokal
      // utkastilstand (en valgt rad i en liste), og den skal tilbake.
      deps.revert();
      return "declined";
    }
  }

  deps.apply();
  receipt("saving");

  const persisted = await deps.persist();
  if (!persisted) {
    deps.revert();
    deps.toast("error", deps.saveFailedMessage());
    receipt("failed");
    return "failed";
  }

  receipt("saved");
  await deps.after?.();
  return "saved";
}
