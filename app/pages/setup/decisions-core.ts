/**
 * De fem beslutningene — hva som er svart, hva svaret ER, og om det holder.
 *
 * ## Hvorfor dette er en ren fil
 *
 * Nivå 1 av Oppsett er én påstand per spørsmål, og hver påstand kan være
 * usann på en måte ingen oppdager før en søndag. Atlaset fant den verste i
 * dagens app: enhetskortet skrev «Innebygd mikrofon · Tilkoblet ✓» når
 * `deviceId` var `null`, altså «alt er i orden» om en innstilling som ikke er
 * satt. Regelen som hindrer det kan ikke bo i en komponent — der er den én
 * `&&` i en JSX-linje ingen leser to ganger. Her er den en tabell, og tabellen
 * har en test per rad.
 *
 * Derfor: INGEN i18n her, ingen DOM, ingen `window`. Fila svarer med DATA
 * («enheten er borte, den het X»), og kallstedet oversetter. En kjerne som
 * kalte `t()` ville trukket katalogen inn i node-gaten og gjort hver regel
 * avhengig av hvilken tekst noen skrev sist.
 *
 * ## Tre tilstander, ikke to
 *
 * `done` og `todo` er de to canvasen tegner. Den tredje, `unknown`, finnes
 * fordi den ellers ville blitt løyet bort: enhetslisten og ledig diskplass
 * leses ASYNKRONT etter at siden er malt. Regelen «ikke funnet ⇒ todo» ville
 * derfor gjort hvert eneste kaldstart til et gult kort som blir nøytralt etter
 * 100 ms — og et gult kort som forsvinner av seg selv er nettopp det som lærer
 * folk å ignorere gult. `unknown` er nøytralt, sier ingenting, og er ALDRI
 * `answered`. Fasiten står fast: bare `done` teller som besvart.
 */

import type { EmailFacts, GateStatus } from "@lib/ui/feature-gate-core";

import type { LevelWord } from "../../audio/level-words";
import type { Settings } from "../../state/settings";

/** De fem spørsmålene, i rekkefølgen de stilles. */
export type DecisionId = "sound" | "folder" | "quality" | "church" | "notify";

/** `done` = svart og i orden · `todo` = må gjøres · `unknown` = ikke målt ennå. */
export type DecisionStatus = "done" | "todo" | "unknown";

/** De tre kvalitetskortene. Alt annet er «egendefinert». */
export type QualityId = "mp3" | "flac" | "wav";

/**
 * Svaret som står nå, som data.
 *
 * `key` navngir SETNINGEN kallstedet skal slå opp; feltene er innsettingene.
 * En ferdig streng her ville betydd at kjernen valgte språk.
 */
export type Answer =
  | { key: "notSetUp" }
  | { key: "device"; name: string; pair: ChannelPair | null }
  | { key: "deviceMissing"; name: string }
  | { key: "path"; path: string }
  | { key: "quality"; format: QualityId }
  | { key: "qualityCustom"; format: string; bitrate: string }
  | { key: "church"; name: string }
  | { key: "nobody" }
  | { key: "email"; address: string };

/** Linja under svaret: hvorfor det holder, eller hva som mangler. */
export type Detail =
  | { key: "heard"; word: LevelWord }
  | { key: "deviceGone"; name: string }
  | { key: "noDevice" }
  | { key: "space"; freeBytes: number; roomMinutes: number | null }
  | { key: "noFolder" }
  | { key: "qualityDesc"; format: QualityId }
  | { key: "qualityCustomDesc" }
  | { key: "language"; language: string }
  | { key: "nobodyDesc" }
  | { key: "emailDesc" };

/** Kanalparet en flerkanals enhet tar opp fra. 1-indeksert — brukeren teller
 *  fra 1, miksebordet er merket fra 1, og bare koden teller fra 0. */
export interface ChannelPair {
  l: number;
  r: number;
}

export interface Decision {
  id: DecisionId;
  /** Bare `done`. `unknown` er ikke et halvt ja. */
  answered: boolean;
  status: DecisionStatus;
  answer: Answer;
  detail: Detail | null;
}

/** Én lydenhet, slik beslutningen trenger den. */
export interface DeviceFact {
  id: string;
  name: string;
  /** Antall inngangskanaler. 0 = ukjent. */
  channels: number;
}

export interface DecisionFacts {
  settings: Settings;
  /** Enhetene bakenden ser. `null` = ikke lest ennå (ikke «ingen»). */
  devices: readonly DeviceFact[] | null;
  /** Ledige byte på lagringsdisken. `null` = ikke lest ennå. */
  diskFreeBytes: number | null;
  /** Minutter opptak det er plass til, eller `null`. */
  roomMinutes: number | null;
  /**
   * Finnes det en vei ut for en e-post? (`hasEmailTransport` — bygget med
   * e-postfeaturen, SMTP-vert og brukernavn utfylt, passord i nøkkelringen.)
   * `null` = ikke lest ennå.
   */
  emailTransport: boolean | null;
  /**
   * Språket appen FAKTISK rendrer i (`app/i18n`s `locale`), ikke det som står
   * i `settings.language`.
   *
   * Fem av de sju katalogene er pauset gjennom redesignet, så en profil som
   * satte tysk leser engelsk på skjermen. Et kirkekort som svarte «Språk:
   * tysk» ville sagt noe brukeren kan se med egne øyne at ikke stemmer — og
   * ville dessuten slått opp en katalognøkkel `app/` ikke har.
   */
  locale: string;
  /** Hva måleren hører i dette øyeblikket, eller `null` når ingen lytter. */
  vuWord: LevelWord | null;
}

function decision(
  id: DecisionId,
  status: DecisionStatus,
  answer: Answer,
  detail: Detail | null,
): Decision {
  return { id, status, answered: status === "done", answer, detail };
}

/** Kanalparet som er lagret for denne enheten, 1-indeksert. `null` = ingen. */
export function channelPairFor(
  settings: Settings,
  deviceId: string,
): ChannelPair | null {
  const stored = settings.deviceChannels?.[deviceId];
  if (!stored) return null;
  const l = stored.channelL;
  const r = stored.channelR;
  if (!Number.isFinite(l) || !Number.isFinite(r)) return null;
  return { l: l + 1, r: r + 1 };
}

/**
 * 1 — Hvilken lyd?
 *
 * `deviceId: null` er ALDRI besvart, uansett hva `deviceName` sier. Det er den
 * ene regelen atlaset ba om ved navn: dagens enhetskort maler «Tilkoblet ✓» på
 * vertsstandarden når ingenting er valgt, og en frivillig som har lest det tror
 * spørsmålet er ferdig.
 *
 * Og et valg som ikke finnes lenger er heller ikke besvart: mikseren som ble
 * skrudd av i går er nøyaktig det som gjør at søndagen blir stille.
 */
export function decideSound(facts: DecisionFacts): Decision {
  const stored = (facts.settings.deviceId ?? "").trim();
  const label = (facts.settings.deviceName ?? "").trim() || stored;

  if (!stored) {
    return decision("sound", "todo", { key: "notSetUp" }, { key: "noDevice" });
  }
  if (facts.devices === null) {
    // Listen er ikke lest. Vi vet hva som er VALGT, men ikke om den finnes —
    // så vi sier det ene og påstår ikke det andre.
    return decision(
      "sound",
      "unknown",
      { key: "device", name: label, pair: null },
      null,
    );
  }

  const hit = facts.devices.find((d) => d.id === stored);
  if (!hit) {
    return decision(
      "sound",
      "todo",
      { key: "deviceMissing", name: label },
      { key: "deviceGone", name: label },
    );
  }

  // Kanalparet hører til SVARET, ikke til detaljen: «Behringer X32 · kanal
  // 15–16» er ett svar på ett spørsmål. Bare for enheter der valget finnes —
  // et stereokort har ingen kanaler å velge mellom.
  const pair = hit.channels > 2 ? channelPairFor(facts.settings, stored) : null;
  const detail: Detail | null = facts.vuWord
    ? { key: "heard", word: facts.vuWord }
    : null;
  return decision(
    "sound",
    "done",
    { key: "device", name: hit.name, pair },
    detail,
  );
}

/**
 * Kanalparene en enhet med `count` kanaler tilbyr: 1–2, 3–4, … Verdien er den
 * 0-indekserte VENSTRE kanalen, som er slik `deviceChannels` lagrer den.
 *
 * Par og ikke enkeltkanaler, fordi lyd kommer i par ut av et miksebord — og
 * den som velger «15» og «16» hver for seg kan velge «15» og «3». En odde
 * siste kanal faller ut med vilje: den har ingen partner å være høyre til.
 */
export function channelPairs(count: number): number[] {
  const pairs: number[] = [];
  for (let i = 0; i + 1 < count; i += 2) pairs.push(i);
  return pairs;
}

/**
 * 2 — Hvor skal opptakene?
 *
 * En mappe uten et svar fra `get_disk_space` er ikke bevis for at det er plass.
 * Men det er heller ikke bevis for at det ikke er det, så det er `unknown` og
 * ikke `todo` (se toppen av fila).
 */
export function decideFolder(facts: DecisionFacts): Decision {
  const folder = (facts.settings.saveFolder ?? "").trim();
  if (!folder) {
    return decision("folder", "todo", { key: "notSetUp" }, { key: "noFolder" });
  }
  if (facts.diskFreeBytes === null) {
    return decision("folder", "unknown", { key: "path", path: folder }, null);
  }
  return decision(
    "folder",
    "done",
    { key: "path", path: folder },
    {
      key: "space",
      freeBytes: facts.diskFreeBytes,
      roomMinutes: facts.roomMinutes,
    },
  );
}

/**
 * Hvilket av de tre kortene innstillingene svarer til — `null` når de ikke
 * svarer til noe av dem.
 *
 * `null` er ikke en feil. Den gamle appen har fire formater og tre bitrater,
 * og en profil som ble satt der (eller importert fra en annen maskin) er en
 * gyldig kombinasjon vi ikke har et kort for. Da sier kortet hva den ER i
 * stedet for å hake av «God» og stille flytte brukeren dit ved neste lagring.
 */
export function qualityIdFor(settings: Settings): QualityId | null {
  const format = String(settings.format ?? "mp3").toLowerCase();
  if (format === "flac") return "flac";
  if (format === "wav") return "wav";
  if (format === "mp3" && String(settings.bitrate ?? "256") === "256") {
    return "mp3";
  }
  return null;
}

/** 3 — Hvilken kvalitet? Alltid besvart: standarden ER et svar. */
export function decideQuality(facts: DecisionFacts): Decision {
  const id = qualityIdFor(facts.settings);
  if (id) {
    return decision(
      "quality",
      "done",
      { key: "quality", format: id },
      { key: "qualityDesc", format: id },
    );
  }
  return decision(
    "quality",
    "done",
    {
      key: "qualityCustom",
      format: String(facts.settings.format ?? "mp3").toUpperCase(),
      bitrate: String(facts.settings.bitrate ?? ""),
    },
    { key: "qualityCustomDesc" },
  );
}

/**
 * 4 — Hvilken kirke? Språket vises alltid, også når navnet mangler.
 *
 * Språket er en del av svaret og ikke et eget spørsmål: det er den ene
 * innstillingen på denne skjermen som endrer alt en frivillig leser, og et
 * kirkekort som ikke nevnte den ville betydd at «hvor bytter jeg språk?» ikke
 * har noe svar på nivå 1.
 */
export function decideChurch(facts: DecisionFacts): Decision {
  const name = (facts.settings.churchName ?? "").trim();
  const language: Detail = { key: "language", language: facts.locale };
  if (!name) return decision("church", "todo", { key: "notSetUp" }, language);
  return decision("church", "done", { key: "church", name }, language);
}

/**
 * 5 — Hvem får beskjed hvis noe går galt?
 *
 * ## Hvorfor bryteren alene ikke er nok
 *
 * `notifyStart`/`notifyStop` varsler PÅ MASKINEN. Det er ekte og nyttig, men
 * det svarer ikke på spørsmålet kortet stiller: hvis opptaket stopper klokka
 * 11:42 og ingen sitter ved maskinen, får ingen vite det. Så kortet er besvart
 * bare når en e-post faktisk kan komme fram — adresse, bryter PÅ, og en
 * sendevei som finnes.
 *
 * ## Hvorfor det ikke finnes et «bare varsel på maskinen»-svar
 *
 * Det ville krevd en ny innstillingsnøkkel i Rust for å huske valget, og en
 * nøkkel legges ikke til fordi et kort gjerne vil bli grønt. Uten et sted å
 * lagre valget ville kortet blitt «ferdig» av noe ingen kan lese tilbake ved
 * neste oppstart. Så det står som `todo`, og teksten sier sant: ingen får
 * e-post, maskinen varsler bare den som sitter ved den.
 */
export function decideNotify(facts: DecisionFacts): Decision {
  const address = (facts.settings.emailAddress ?? "").trim();
  const on = facts.settings.emailOnError === true;

  if (facts.emailTransport === null && on && address) {
    // Alt brukeren kan se er på plass; om det finnes en sendevei vet vi ikke
    // ennå. Ingen påstand i noen retning.
    return decision("notify", "unknown", { key: "email", address }, null);
  }
  if (on && address && facts.emailTransport === true) {
    return decision(
      "notify",
      "done",
      { key: "email", address },
      { key: "emailDesc" },
    );
  }
  return decision("notify", "todo", { key: "nobody" }, { key: "nobodyDesc" });
}

/**
 * Hva e-postbryteren på spørsmål 5 har lov til å gjøre.
 *
 * TRE utfall, ikke to. `emailGateStatus` i `@lib/ui/feature-gate-core` svarer
 * bare på det første — og skriver ned hvorfor: den brukes på et kort som HAR
 * SMTP-feltene i seg, og en gate som slår av sine egne oppsettsfelter kan aldri
 * konfigureres. Her bor SMTP-feltene under Avansert, på en annen skjerm, så
 * mellomtilstanden er både trygg og den mest nyttige: «det finnes en sendevei i
 * denne utgaven, men ingen server er satt opp — og her er hvor du gjør det».
 *
 * `null` (ikke lest ennå) er `ok`: en bryter som er inert i det halvsekundet
 * det tar å spørre bakenden er en bryter som ikke tar imot det første klikket.
 */
export function notifyGateStatus(facts: EmailFacts | null): GateStatus {
  if (facts === null) return "ok";
  if (!facts.featureBuilt) return "unavailable";
  if (!facts.smtpConfigured || !facts.smtpPasswordAvailable) {
    return "unconfigured";
  }
  return "ok";
}

/** Alle fem, i rekkefølge. Nummeret på kortet er indeksen + 1. */
export function decisionsFor(facts: DecisionFacts): Decision[] {
  return [
    decideSound(facts),
    decideFolder(facts),
    decideQuality(facts),
    decideChurch(facts),
    decideNotify(facts),
  ];
}

/**
 * Sier knappen «Sett opp» eller «Endre»?
 *
 * IKKE det samme som `answered`. Et kort kan stå på `unknown` — mappen er
 * valgt, men disken har ikke svart ennå — og da finnes det noe å ENDRE selv om
 * spørsmålet ikke er kvittert ut. «Sett opp» på en mappe som allerede er satt
 * opp er en knapp som beskriver skjermen feil.
 *
 * Så: «Sett opp» bare når det bokstavelig talt ikke står et svar.
 */
export function needsSetUp(decision: Decision): boolean {
  return decision.answer.key === "notSetUp" || decision.answer.key === "nobody";
}

/** Hvor mange av de fem som faktisk er besvart. */
export function answeredCount(decisions: readonly Decision[]): number {
  return decisions.filter((d) => d.answered).length;
}
