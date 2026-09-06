/**
 * `useSetting(key)` — én innstilling, én lagringsmodell, én kvittering.
 *
 * Alt auto-anvender med en synlig kvittering. Legacy hadde tre modeller
 * samtidig (stille auto-lagring på lyd/video, en skitten-fot med
 * Lagre/Avbryt der «Avbryt» ikke kunne angre noe som helst, og tre innebygde
 * «Lagre»-knapper med tre forskjellige etiketter) — en frivillig hadde ingen
 * måte å vite hvilken flate hun stod på.
 *
 * Beslutningene er ikke her. NÅR en endring committes og HVILKEN verdi som
 * committes kommer fra `@lib/ui/bind-setting-core` (`planCommit`,
 * `coerceValue`, `isRealChange`, `validateNumber`); HVA som skjer etterpå
 * kommer fra `use-setting-core.ts`, som er tabelltestet over hele sekvensen.
 * Denne fila er hooken rundt dem.
 *
 * ## Ingen commit forsvinner stille
 *
 * Blir hooken bedt om å committe mens en commit går, KØES den og kjøres når
 * den første er ferdig — med utkastet lest på nytt, så siste verdi er den som
 * lander og kvitteres for. Overgangen er ren og tabelltestet
 * (`stepCommitQueue`). Kontrollene låses ikke mens skrivningen går: et
 * tekstfelt som slår seg av midt i en setning er en flate som straffer
 * brukeren for å skrive fort, og køen gjør låsen unødvendig.
 *
 * ## `resyncBoundSettings` finnes ikke
 *
 * Legacy måtte holde et eget «forrige verdi»-grunnlag per binding, og
 * oppfriske det hver gang en `applyXSettingsToUI()`-runde hadde skrevet om
 * DOM-en — ellers ville en kontroll brukeren satte TILBAKE til verdien den
 * hadde da siden ble koblet sammenlignet likt og aldri blitt skrevet. Her er
 * grunnlaget alltid `settings.value[key]`, altså den lagrede verdien selv.
 * Det er ingenting å synkronisere, og derfor ingenting å glemme.
 */

import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import {
  coerceValue,
  planCommit,
  type ControlKind,
  type SettingValue,
} from "@lib/ui/bind-setting-core";

import { t } from "../i18n";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
  type Settings,
} from "../state/settings";
import { confirmDialog } from "../ui/dialog";
import { toast as showToast } from "../ui/toast";
import { useReceipt } from "./use-receipt";
import {
  IDLE_COMMIT_QUEUE,
  narrowToStored,
  runCommit,
  stepCommitQueue,
  type CommitQueue,
  type GuardDescriptor,
  type Receipt,
} from "./use-setting-core";

/** Innstillingsnøkler som holder én verdi (ikke lister eller kart). */
export type ScalarSettingKey = {
  [K in keyof Settings]: Settings[K] extends SettingValue ? K : never;
}[keyof Settings];

export interface UseSettingOpts<T> {
  /**
   * Kontrolltypen, som bestemmer commit-planen (bryter/select committer på
   * `change`, glidebryter på slipp, fritekst med etterslep). Utledes fra
   * verdien når den ikke er oppgitt — en boolean er en bryter, et tall er et
   * tallfelt — så det vanlige tilfellet ikke trenger å si det.
   */
  kind?: ControlKind;
  /** Commit på hendelsen selv, også for et tekstfelt. */
  immediate?: boolean;
  /** Eget etterslep, som slår kontrolltypens standard. */
  debounceMs?: number;
  /** Smalne råverdien før validering. */
  coerce?: (raw: SettingValue) => T;
  /** Returner en melding for å avvise verdien. */
  validate?: (value: T) => string | null;
  /** Returner en beskrivelse for å spørre først — f.eks. `recordingImminentGuard`. */
  confirmIf?: (value: T) => GuardDescriptor | null;
  /** Kjøres etter en vellykket lagring. */
  after?: (value: T) => void;
  /** Still spørsmålet. Injisert så sekvensen kan drives uten en dialogvert. */
  confirm?: (guard: GuardDescriptor) => Promise<boolean>;
  /** Si fra at det gikk galt. Injisert av samme grunn. */
  toast?: (kind: "error", msg: string) => void;
  /** Persister. Standard er den delte etterslepende lagringen. */
  persist?: () => Promise<boolean>;
}

export interface UseSettingResult<T> {
  /** Den LAGREDE verdien. */
  value: T;
  /** Verdien kontrollen står på nå — kan være midt i en redigering. */
  draft: SettingValue;
  /** Brukeren endret kontrollen. Committer selv etter kontrolltypens plan. */
  set: (next: SettingValue) => void;
  /** Commit nå (blur, Enter, en knapp som lagrer). */
  commit: () => Promise<void>;
  receipt: Receipt;
  /** Valideringsfeilen, som vises under feltet. */
  error: string | null;
  /** En dialog eller en skrivning er i lufta — kontrollen skal ikke ta imot mer. */
  busy: boolean;
  /**
   * DOM-hendelsene kontrollen skal lytte på, fra `planCommit`. TILLEGG til det
   * dirigenten spesifiserte: uten den ville S1b måtte gjette hvilke hendelser
   * som hører til hvilken kontrolltype, og et debouncet felt som bare lytter
   * på `change` committer aldri en verdi som skrives og forlates.
   */
  events: readonly string[];
}

/** Kontrolltypen en verdi oppfører seg som, når ingen sa noe annet. */
function kindOfValue(value: SettingValue): ControlKind {
  if (typeof value === "boolean") return "toggle";
  if (typeof value === "number") return "number";
  return "text";
}

export function useSetting<K extends ScalarSettingKey>(
  key: K,
  opts: UseSettingOpts<Settings[K]> = {},
): UseSettingResult<Settings[K]> {
  // Lesningen som abonnerer: en lagring hvor som helst i appen oppdaterer
  // denne kontrollen.
  const value = settings.value[key] as SettingValue;

  const [draft, setDraft] = useState<SettingValue>(value);
  const { receipt, show: showReceipt } = useReceipt();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const draftRef = useRef<SettingValue>(draft);
  // Køen, ikke et bart flagg — se `stepCommitQueue` i use-setting-core.ts.
  const queue = useRef<CommitQueue>(IDLE_COMMIT_QUEUE);
  const editingRef = useRef(false);
  const commitTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const optsRef = useRef(opts);
  optsRef.current = opts;

  const plan = planCommit(opts.kind ?? kindOfValue(value), {
    immediate: opts.immediate,
    debounceMs: opts.debounceMs,
  });

  // En endring som kom UTENFRA (en importert profil, en annen flate) skal
  // vises — men aldri midt i en redigering, for da ville vi slettet det
  // brukeren holdt på å skrive.
  useEffect(() => {
    if (!editingRef.current) {
      draftRef.current = value;
      setDraft(value);
    }
  }, [value]);

  // Timeren overlever ikke at kontrollen forsvinner. (Kvitteringens egen
  // nedtelling ryddes av `useReceipt`.)
  useEffect(
    () => () => {
      if (commitTimer.current) clearTimeout(commitTimer.current);
    },
    [],
  );

  /**
   * Commit NÅ — og aldri stille tapt.
   *
   * Er en commit allerede i lufta, legges denne i køen og kjøres av den som
   * går, med utkastet lest på nytt. Den gamle utgaven ryddet den ventende
   * timeren FØR den sjekket `busy` og returnerte så tomhendt: redigeringen
   * brukeren nettopp gjorde forsvant, og skjermen ble stående og påstå en
   * verdi basen ikke hadde. Se `stepCommitQueue` i `use-setting-core.ts` for
   * hvorfor køen og ikke bare en omstokket rekkefølge.
   */
  const commit = useCallback(async (): Promise<void> => {
    if (commitTimer.current) {
      clearTimeout(commitTimer.current);
      commitTimer.current = null;
    }
    const asked = stepCommitQueue(queue.current, "request");
    queue.current = asked.next;
    // Én som går tar denne med seg — den leser utkastet på nytt når den er
    // ferdig, så det er alltid SISTE verdi som lander.
    if (asked.action !== "run") return;
    setBusy(true);

    try {
      for (;;) {
        const o = optsRef.current;
        // Grunnlaget er den LAGREDE verdien, lest i det øyeblikket vi
        // committer — ikke et hurtiglager fra da kontrollen ble koblet.
        const previous = settings.peek()[key] as SettingValue;

        await runCommit<Settings[K]>({
          previous,
          next: draftRef.current,
          // Standarden smalner mot den LAGREDE typen — se `narrowToStored`. Et
          // `<select>` leverer alltid en streng, og «30» der Rust venter `i32`
          // avviser hele lagringen.
          coerce:
            o.coerce ?? ((raw) => narrowToStored(previous, raw) as Settings[K]),
          validate: o.validate,
          confirmIf: o.confirmIf,
          confirm:
            o.confirm ?? ((guard) => confirmDialog({ ...guard, danger: true })),
          apply: (v) => patchSettings({ [key]: v } as Partial<Settings>),
          persist: o.persist ?? (() => saveSettingsDebounced()),
          revert: (prev) => {
            patchSettings({ [key]: prev } as Partial<Settings>);
            draftRef.current = prev;
            setDraft(prev);
          },
          toast: o.toast ?? ((kind, msg) => showToast(kind, msg)),
          saveFailedMessage: () => t("general.saveFailed"),
          after: o.after,
          onError: setError,
          // «Lagret ✓» er en kvittering, ikke en tilstand — `useReceipt`
          // teller den ned. «Mislyktes» blir stående til noe skjer, for den
          // er ikke lest ennå.
          onReceipt: showReceipt,
        });

        const settled = stepCommitQueue(queue.current, "settled");
        queue.current = settled.next;
        if (settled.action !== "run") break;
      }
    } finally {
      // Skulle `runCommit` kaste, står ikke køen igjen som «opptatt for
      // alltid» — en kontroll som aldri kan committe igjen er verre enn den
      // feilen som kastet.
      queue.current = IDLE_COMMIT_QUEUE;
      editingRef.current = false;
      setBusy(false);
    }
  }, [key, showReceipt]);

  const set = useCallback(
    (next: SettingValue): void => {
      editingRef.current = true;
      draftRef.current = next;
      setDraft(next);
      if (commitTimer.current) clearTimeout(commitTimer.current);
      if (plan.debounceMs === 0) {
        void commit();
        return;
      }
      commitTimer.current = setTimeout(() => {
        commitTimer.current = null;
        void commit();
      }, plan.debounceMs);
    },
    [commit, plan.debounceMs],
  );

  return {
    value: value as Settings[K],
    draft,
    set,
    commit,
    receipt,
    error,
    busy,
    events: plan.events,
  };
}

/** Gjenbruk av kjernens coercion for en rå DOM-verdi, så en komponent slipper
 *  å importere to steder fra. */
export { coerceValue };
