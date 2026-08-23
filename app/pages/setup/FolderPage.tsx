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
 * ## Native dialog, ikke et tekstfelt
 *
 * `window.api.pickFolder` åpner OS-ets egen mappevelger. Det er ikke bare
 * hyggeligere enn å skrive en sti: dialogen ER autorisasjonen — bakenden
 * håndhever at appen bare skriver til steder brukeren faktisk har pekt på.
 */

import { useState } from "preact/hooks";

import { t, tf } from "../../i18n";
import {
  currentRoomMinutes,
  diskFreeBytes,
  refreshDiskSpace,
} from "../../state/disk";
import {
  patchSettings,
  saveSettingsDebounced,
  settings,
} from "../../state/settings";
import { Button } from "../../ui/Button/Button";
import { Card } from "../../ui/Card/Card";
import { EmptyState } from "../../ui/EmptyState/EmptyState";
import { Receipt } from "../../ui/Receipt/Receipt";
import { toast } from "../../ui/toast";
import type { Receipt as ReceiptState } from "../../settings/use-setting-core";
import styles from "./setup.module.css";
import { SubPage } from "./SubPage";

export function FolderPage() {
  const folder = (settings.value.saveFolder ?? "").trim();
  const [receipt, setReceipt] = useState<ReceiptState>("idle");
  const [busy, setBusy] = useState(false);

  async function pick(): Promise<void> {
    if (busy) return;
    setBusy(true);
    try {
      const chosen = await window.api.pickFolder();
      // Avbrutt dialog: ingen endring, ingen kvittering. En «Lagret ✓» her
      // ville vært en kvittering for noe som ikke skjedde.
      if (!chosen) return;
      setReceipt("saving");
      patchSettings({ saveFolder: chosen });
      const ok = await saveSettingsDebounced(120);
      setReceipt(ok ? "saved" : "failed");
      if (!ok) {
        toast("error", t("general.saveFailed"));
        return;
      }
      // Ny disk, nytt tall: plassen på den gamle mappen sier ingenting om den
      // nye, og «plass til 300 t» må ikke bli stående fra forrige valg.
      await refreshDiskSpace();
    } finally {
      setBusy(false);
    }
  }

  const pickButton = (
    <Button
      variant={folder ? "secondary" : "primary"}
      busy={busy}
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
