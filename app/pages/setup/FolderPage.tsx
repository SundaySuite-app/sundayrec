/**
 * 2 — Hvor skal opptakene?
 *
 * Én sti, én knapp. Navnemønster, autosletting og oppdeling er Avansert
 * (P1b) — de er ting «noen» trenger, ikke ting alle må svare på før første
 * søndag.
 *
 * ## Plass i TIMER
 *
 * «412 GB ledig» svarer ikke på spørsmålet. «Plass til 300 t» gjør det, og det
 * er det samme tallet statuslinjen bruker for å kunne si «Lite plass igjen» før
 * det er for sent. Regnestykket er `app/state/disk.ts`, delt med statuslinjen —
 * to anslag som er «omtrent like» ville betydd at kortet og skinnen kan si
 * forskjellige ting om den samme disken.
 *
 * ## Native dialog, ikke et tekstfelt — og Rust åpner den
 *
 * `window.api.settingsPickSaveFolder` ber RUST åpne OS-ets egen mappevelger,
 * sjekke mappa og lagre den. Det er ikke bare hyggeligere enn å skrive en sti:
 * dialogen ER autorisasjonen, og siden den åpnes av prosessen og ikke av
 * nettvisningen kan ingen sti sendes derfra uten at en dialog ble vist.
 * Før (A2-familien) åpnet siden velgeren selv og sendte svaret tilbake i
 * `settings_save` — og med den samme kommandoen kunne et kompromittert
 * webview satt opptaksmappa til hvilken som helst mappe uten noen dialog.
 * `settings_save` rører ikke mappa lenger; svaret her er de lagrede
 * innstillingene, og siden speiler dem.
 *
 * ## Når bakenden sier nei
 *
 * En NY mappe sjekkes før den lagres: ikke en app eller pakke, ikke roten av
 * disken eller selve hjemmemappa, ikke `~/.ssh` og slike. Ingenting lagres, og
 * toasten sier hvorfor (`folder-refusal.ts`). En mappe som allerede står
 * lagret, sjekkes aldri.
 */

import { useState } from "preact/hooks";

import { errorCode } from "@lib/error-code-core";

import { t, tf } from "../../i18n";
import { useReceipt } from "../../settings/use-receipt";
import {
  currentRoomMinutes,
  diskFreeBytes,
  refreshDiskSpace,
} from "../../state/disk";
import { patchSettings, settings } from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { Card } from "../../ui/Card/Card";
import { EmptyState } from "../../ui/EmptyState/EmptyState";
import { Receipt } from "../../ui/Receipt/Receipt";
import { toast } from "../../ui/toast";
import { folderRefusalMessage } from "./folder-refusal";
import styles from "./setup.module.css";
import { SubPage } from "./SubPage";

export function FolderPage() {
  const folder = (settings.value.saveFolder ?? "").trim();
  // Kvitteringen teller ned av seg selv (`useReceipt`). Selve lagringen er
  // Rusts: svaret på valget ER det som ble lagret, så det finnes ingenting å
  // rulle tilbake — en avvisning har ikke rørt noe.
  const { receipt, show, reset } = useReceipt();
  const [picking, setPicking] = useState(false);

  async function pick(): Promise<void> {
    if (picking) return;
    setPicking(true);
    try {
      const answer = await window.api.settingsPickSaveFolder();
      if (!answer.ok) {
        // Bakenden sa nei til mappa (en app, roten av disken …) — toasten
        // sier hvorfor, ikke bare at det ikke ble lagret.
        toast(
          "error",
          folderRefusalMessage(errorCode(answer.error)) ??
            t("general.saveFailed"),
        );
        show("failed");
        return;
      }
      // Avbrutt dialog: ingen endring, ingen kvittering. En «Lagret ✓» her
      // ville vært en kvittering for noe som ikke skjedde.
      if (!answer.settings) {
        reset();
        return;
      }
      patchSettings({ saveFolder: answer.settings.saveFolder ?? null });
      show("saved");
      // Ny disk, nytt tall: plassen på den gamle mappen sier ingenting om
      // den nye, og «plass til 300 t» må ikke bli stående fra forrige valg.
      await refreshDiskSpace();
    } finally {
      setPicking(false);
    }
  }

  const pickButton = (
    <Button
      variant={folder ? "secondary" : "primary"}
      busy={picking}
      testId="folder-pick"
      onClick={() => void pick()}
    >
      {t("app.setup.folder.pick")}
    </Button>
  );

  return (
    <SubPage lede={t("app.setup.folder.lede")} testId="setup-folder">
      {folder ? (
        <Card
          testId="folder-current"
          title={t("app.setup.folder.label")}
          actions={pickButton}
        >
          <div data-testid="folder-path" class={styles.path}>
            {folder}
          </div>
          <p data-testid="folder-space" class={styles.hint}>
            {spaceText()}
          </p>
          <div class={styles.footer}>
            <Receipt state={receipt} testId="folder-receipt" />
          </div>
        </Card>
      ) : (
        <EmptyState
          testId="folder-empty"
          title={t("app.setup.folder.none")}
          description={t("app.setup.folder.noneDesc")}
          action={pickButton}
        />
      )}
    </SubPage>
  );
}

/** «412 GB ledig · plass til 300 t», eller bare det halve vi vet. */
function spaceText(): string {
  const free = diskFreeBytes.value;
  if (free === null) return t("app.setup.folder.unknownSpace");
  const gb = Math.round(free / 1e9);
  const minutes = currentRoomMinutes();
  if (minutes === null) return tf("app.setup.folder.free", { gb });
  return tf("app.setup.folder.space", { gb, hours: Math.floor(minutes / 60) });
}
