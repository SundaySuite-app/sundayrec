/**
 * Hvorfor bakenden sa nei til en ny opptaksmappe — som en setning.
 *
 * ## Hvorfor det finnes et nei
 *
 * Opptaksmappa er ikke bare der filene havner. Den er mappa menylinjens
 * «Åpne opptaksmappen» åpner, og den avgjør hvilke filer «Vis i Finder» får
 * vise. Derfor sjekker bakenden en NY mappe før den lagres
 * (`vet_new_save_folder` i `src-tauri/src/commands/recordings_open.rs`): ikke
 * en app eller pakke (som macOS ville STARTET i stedet for å vise), ikke roten
 * av disken eller selve hjemmemappa, ikke en beskyttet mappe som `~/.ssh`, og ikke appens egen datamappe (der
 * gjenopprettingsmappa ligger). En
 * mappe som allerede står lagret, sjekkes aldri på nytt — den tar opp som før.
 *
 * ## Hvorfor en egen setning per kode
 *
 * «Kunne ikke lagre innstillingen» er sant, men den frivillige velger da den
 * samme mappa igjen. Setningen må si hva som er galt med akkurat DENNE mappa,
 * og hva hen kan gjøre i stedet. En kode uten setning her (en ny regel i Rust)
 * faller tilbake til den vanlige teksten — og `folder-refusal.test.ts` leser
 * Rust-kilden og feiler før det skjer.
 *
 * Hvert `t()`-kall står med literal nøkkel, slik `check-i18n-keys.mjs` krever.
 */

import { t } from "../../i18n";

/** Setningen for en avvist ny opptaksmappe, eller `null` for en annen feil. */
export function folderRefusalMessage(code: string): string | null {
  switch (code) {
    case "save_folder_is_a_package":
      return t("app.setup.folder.refusedPackage");
    case "save_folder_too_broad":
      return t("app.setup.folder.refusedTooBroad");
    case "save_folder_protected":
      return t("app.setup.folder.refusedProtected");
    case "save_folder_app_data":
      return t("app.setup.folder.refusedAppData");
    case "save_folder_invalid":
      return t("app.setup.folder.refusedInvalid");
    default:
      return null;
  }
}
