/**
 * «Kastes de ulagrede kuttene?» — ett spørsmål, ett sted.
 *
 * To flater stiller det: «Til biblioteket» i editorens topplinje, og et SLIPP
 * på Redigering-siden (som åpner en annen fil oppå den som står). Fram til D3
 * bodde begge i `EditorPage.tsx`, fordi begge var editorens egne. Etter D3 er
 * slippsonen løftet ut til Redigering-siden — den skal ta imot en fil også når
 * biblioteket står — og da måtte spørsmålet flytte hit i stedet for å bli
 * skrevet en gang til.
 *
 * Det bor IKKE i `loader.ts`, av grunnen som står i kommentaren over
 * `closeFile` der: bekreftelsen er en setning en frivillig leser, og modellen
 * har ingen katalog.
 */

import { t } from "../i18n";
import { confirmDialog } from "../ui/dialog";
import { E } from "./model";

/** Spør før ulagrede kutt kastes. Sann = det er trygt å gå videre. */
export async function confirmDiscard(): Promise<boolean> {
  if (!E.dirty) return true;
  return confirmDialog({
    title: t("editor.confirmClose"),
    message: t("dialog.discardEditsBody"),
    confirmLabel: t("dialog.discardEdits"),
    danger: true,
  });
}
