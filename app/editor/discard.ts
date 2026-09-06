/**
 * Spørsmålene som stilles før en åpen fil forlates — ett sted, i den
 * rekkefølgen det haster.
 *
 * To av dem: «Kastes de ulagrede kuttene?» og — siden F2-A-B — «Eksporten
 * pågår. Avbryte den?». Bare ETT av dem vises noen gang, fordi det andre er
 * inneholdt i det: se `confirmDiscard` nedenfor.
 *
 * To flater stiller kutt-spørsmålet: «Til biblioteket» i editorens topplinje,
 * og et SLIPP på Redigering-siden (som åpner en annen fil oppå den som står).
 * Fram til D3 bodde begge i `EditorPage.tsx`, fordi begge var editorens egne.
 * Etter D3 er slippsonen løftet ut til Redigering-siden — den skal ta imot en
 * fil også når biblioteket står — og da måtte spørsmålet flytte hit i stedet
 * for å bli skrevet en gang til.
 *
 * SETNINGENE bor her og ikke i `loader.ts`, av grunnen som står over
 * `closeFile` der: en bekreftelse er noe en frivillig leser, og modellen har
 * ingen katalog. Men HÅNDHEVELSEN av eksport-spørsmålet er lasterens, og det er
 * ikke en selvmotsigelse: en foreldreløs ffmpeg er ikke en setning, det er en
 * prosess modellen selv startet — og «hver flate husker å spørre» er akkurat
 * antakelsen F2-3 var.
 */

import { t } from "../i18n";
import { confirmDialog } from "../ui/dialog";
import { cancelExport, exporting } from "./export";
import { E } from "./model";

/** Spør før ulagrede kutt kastes. Sann = det er trygt å gå videre. */
export async function confirmDiscard(): Promise<boolean> {
  // En PÅGÅENDE eksport stiller sitt eget, STERKERE spørsmål (under, håndhevet
  // av `loader.ts`): den som sier ja til å avbryte en eksport har allerede sagt
  // ja til å forlate redigeringen. To dialoger på rad om den samme
  // beslutningen er nøyaktig hvordan folk klikker «Ja» på noe de ikke leste —
  // `ui/dialog.ts` sier det selv om hvorfor køen serialiserer.
  if (exporting.peek()) return true;
  if (!E.dirty) return true;
  return confirmDialog({
    title: t("editor.confirmClose"),
    message: t("dialog.discardEditsBody"),
    confirmLabel: t("dialog.discardEdits"),
    danger: true,
  });
}

/**
 * Spør før en PÅGÅENDE eksport forlates — og AVBRYT den hvis svaret er ja.
 * Sann = det er trygt å gå videre.
 *
 * Granskningens F2-3: en eksport pågår, brukeren trykker «Til biblioteket» /
 * slipper en ny fil / velger «Åpne i Rediger» — og `resetExport()` satte bare
 * `exporting = false`. Ingen cancel gikk til bakenden. ffmpeg-en fortsatte å
 * male på en fil ingen flate lenger fortalte om, og siden EKSPORTERING samtidig
 * tilbød «Sist redigert · Gjør klar» på den SAMME fila, var neste klikk en
 * andre eksport oppå den første — F2-2, med to prosesser som skriver til det
 * samme kollisjonsfrie navnet.
 *
 * `cancelExport()` og ikke et rått `editorCancelExport()`-kall: den ene setter
 * `cancelling`, svelger et bakendesvar som ikke kommer, og håndterer
 * forberedelsesfasen (der det ikke finnes en ffmpeg å drepe ennå). Awaiten er
 * ikke pynt — bakenden skal ha fått beskjeden FØR tilstanden nullstilles.
 */
export async function confirmAbandonExport(): Promise<boolean> {
  if (!exporting.peek()) return true;
  const ok = await confirmDialog({
    title: t("editor.confirmExportRunning"),
    message: t("dialog.abortExportBody"),
    confirmLabel: t("dialog.abortExport"),
    danger: true,
  });
  if (!ok) return false;
  await cancelExport();
  return true;
}
