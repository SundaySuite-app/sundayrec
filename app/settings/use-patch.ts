/**
 * `usePatch()` — `useSetting`s lagringsmodell, for et valg som er FLERE nøkler.
 *
 * ## Hvorfor den finnes
 *
 * `useSetting` eier én nøkkel, én forrige verdi og én kvittering. Noen valg er
 * ikke én nøkkel:
 *
 *   • enhetsvalget      `deviceId` + `deviceName` + `deviceChannels[id]`
 *   • opptaksmotoren    `classicFfmpegAudio` + `classicDirectshow`
 *   • OS-varselet       `notifyStart` + `notifyStop`
 *   • kameravalget      `videoDeviceName` + `videoDeviceIndex`
 *   • «del opp»/«slett» én tallnøkkel der 0 betyr av, vist som en bryter
 *
 * Tre `useSetting` over de tre nøklene ville betydd tre skrivninger, tre
 * kvitteringer og et vindu der basen holder halve valget. Så hver av dem skrev
 * sin egen lagringsmodell for hånd — patch, lagre, kvittering, tilbakerulling,
 * toast — litt forskjellig hver gang. Motorvalget hadde ingen nedtelling på
 * kvitteringen og leste `before` fra en render som kunne være foreldet;
 * kameravelgeren rullet ikke tilbake i det hele tatt. Det er skjøtefeilens form
 * med seks eksemplarer.
 *
 * Sekvensen selv bor i `use-setting-core.ts` (`runPatchCommit`) og er
 * tabelltestet, akkurat som `runCommit`. Denne fila er hooken rundt den:
 * kvitteringen (`useReceipt`), `busy`, og den ene tilbakerullingen som alltid
 * er riktig — øyeblikksbildet av de nøklene patchen faktisk rører, lest fra
 * `settings.peek()` i det øyeblikket vi skriver.
 *
 * ## Tilbakerullingen leses NÅ, ikke ved render
 *
 * `settings.peek()[key]` i `write()` og ikke `settings.value[key]` i
 * komponentkroppen: en render kan være fra før en annen flate skrev, og å rulle
 * tilbake til en foreldet verdi er å innføre nøyaktig den løgnen
 * tilbakerullingen finnes for å stoppe.
 */

import { useCallback, useRef, useState } from "preact/hooks";

import { t } from "../i18n";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
  type Settings,
} from "../state/settings";
import { toast as showToast } from "../ui/toast";
import { useReceipt } from "./use-receipt";
import { runPatchCommit, type CommitOutcome } from "./use-setting-core";

/** Vinduet den delte skrivningen samler kall i. Samme tall alle de håndlagde
 *  kallstedene brukte, og av samme grunn: en bryter som avdekker en select man
 *  setter med én gang skal bli én rundtur, ikke to. */
const PATCH_SAVE_MS = 120;

export interface PatchOpts {
  /**
   * Spør først. Returner `false` for å avlyse — `recordingImminentGuard`s
   * imperative søster `confirmIfRecordingImminent` er den vanlige.
   */
  confirm?: () => Promise<boolean>;
  /** Kjøres etter en vellykket lagring (les enhetslisten på nytt, forlik
   *  forhåndsbufferen, naviger videre). */
  after?: () => void | Promise<void>;
  /**
   * Ingen reell endring? Standarden sammenligner patchens nøkler mot det som
   * står lagret; oppgi den bare der «endret» betyr noe annet enn feltlikhet.
   */
  changed?: boolean;
}

export interface UsePatchResult {
  receipt: ReturnType<typeof useReceipt>["receipt"];
  /** Nullstill kvitteringen — brukeren begynte på noe nytt. */
  reset: () => void;
  /** En skrivning er i lufta. */
  busy: boolean;
  /** Skriv patchen. Løses med `true` bare når skrivningen faktisk landet. */
  write: (patch: Partial<Settings>, opts?: PatchOpts) => Promise<boolean>;
}

/** Er noen av patchens nøkler forskjellig fra det som står lagret? */
function differsFromStored(patch: Partial<Settings>): boolean {
  const current = settings.peek() as Record<string, unknown>;
  return Object.entries(patch).some(
    ([key, value]) => !Object.is(current[key], value),
  );
}

/** Øyeblikksbildet av NØYAKTIG de nøklene patchen rører. */
function snapshotOf(patch: Partial<Settings>): Partial<Settings> {
  const current = settings.peek() as Record<string, unknown>;
  const before: Record<string, unknown> = {};
  for (const key of Object.keys(patch)) before[key] = current[key];
  return before as Partial<Settings>;
}

export function usePatch(): UsePatchResult {
  const { receipt, show, reset } = useReceipt();
  const [busy, setBusy] = useState(false);
  // En andre skrivning mens den første går ville skrevet over
  // tilbakerullingens øyeblikksbilde med en tilstand som allerede er halvveis
  // anvendt. Ett kall om gangen; kontrollene er slått av mens `busy` står.
  const busyRef = useRef(false);

  const write = useCallback(
    async (
      patch: Partial<Settings>,
      opts: PatchOpts = {},
    ): Promise<boolean> => {
      if (busyRef.current) return false;
      busyRef.current = true;
      setBusy(true);
      const before = snapshotOf(patch);
      try {
        const outcome: CommitOutcome = await runPatchCommit({
          changed: opts.changed ?? differsFromStored(patch),
          confirm: opts.confirm,
          apply: () => patchSettings(patch),
          persist: () => saveSettingsDebounced(PATCH_SAVE_MS),
          revert: () => patchSettings(before),
          toast: (kind, msg) => showToast(kind, msg),
          saveFailedMessage: () => t("general.saveFailed"),
          after: opts.after,
          onReceipt: show,
        });
        return outcome === "saved";
      } finally {
        busyRef.current = false;
        setBusy(false);
      }
    },
    [show],
  );

  return { receipt, reset, busy, write };
}
