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
  SAVED_CHIP_MS,
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
import {
  narrowToStored,
  runCommit,
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
  const [receipt, setReceipt] = useState<Receipt>("idle");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const draftRef = useRef<SettingValue>(draft);
  const busyRef = useRef(false);
  const editingRef = useRef(false);
  const commitTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const chipTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
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

  // Timere overlever ikke at kontrollen forsvinner.
  useEffect(
    () => () => {
      if (commitTimer.current) clearTimeout(commitTimer.current);
      if (chipTimer.current) clearTimeout(chipTimer.current);
    },
    [],
  );

  const commit = useCallback(async (): Promise<void> => {
    if (commitTimer.current) {
      clearTimeout(commitTimer.current);
      commitTimer.current = null;
    }
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    const o = optsRef.current;
    // Grunnlaget er den LAGREDE verdien, lest i det øyeblikket vi committer —
    // ikke et hurtiglager fra da kontrollen ble koblet.
    const previous = settings.peek()[key] as SettingValue;

    try {
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
        onReceipt: (next) => {
          setReceipt(next);
          if (chipTimer.current) clearTimeout(chipTimer.current);
          // «Lagret ✓» er en kvittering, ikke en tilstand — den forsvinner.
          // «Mislyktes» blir stående til noe skjer, for den er ikke lest ennå.
          if (next === "saved") {
            chipTimer.current = setTimeout(
              () => setReceipt("idle"),
              SAVED_CHIP_MS,
            );
          }
        },
      });
    } finally {
      editingRef.current = false;
      busyRef.current = false;
      setBusy(false);
    }
  }, [key]);

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
