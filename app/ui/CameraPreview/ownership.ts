/**
 * Hvem eier kameraet — og hvordan noen andre ber om å få det.
 *
 * ## Regelen maskinvaren setter
 *
 * macOS gir ÉN klient om gangen tilgang til et kamera. Previewen på Opptak er
 * en `getUserMedia`-strøm i webviewet; opptaket er bakendens ffmpeg. De to kan
 * ikke ha enheten samtidig, og previewen er den som må gi seg.
 *
 * ## Hvorfor ikke bare la `isRecording` gjøre jobben
 *
 * Previewen slipper også kameraet når `isRecording` blir sann — men det
 * signalet settes ETTER at `start_recording` har svart ja
 * (`markSessionStarted`, se `state/recording.ts`). I det øyeblikket har
 * opptakeren allerede prøvd å åpne enheten mens previewen holdt den. Rekkefølgen
 * er hele poenget: SLIPP, og så start.
 *
 * ## Derfor et PAR, og ikke bare en stopp
 *
 * `start_recording` kan svare nei. Skjedde det, ble det ikke noe opptak, og en
 * preview som ble stående svart etter et mislykket trykk er en app som virker
 * ødelagt av å ha sagt fra. `resumeCameraPreview()` er den andre halvdelen, og
 * `handleStart` kaller den i `finally` når ingen økt ble startet.
 *
 * Et SETT og ikke én enkelt funksjon: to previews i treet samtidig er ikke noe
 * designet forutsetter, men et register som stille glemte den ene ville
 * etterlatt et kamera åpent, og det er nøyaktig feilen dette finnes for.
 */

/** En levende preview, sett fra utsiden: den kan slippe, og ta igjen. */
export interface CameraPreviewOwner {
  /** Slipp enheten NÅ. Må tåle å bli kalt når det ikke er noe å slippe. */
  stop: () => void;
  /** Ta den igjen — brukes bare når slippet viste seg å være unødvendig. */
  resume: () => void;
}

const owners = new Set<CameraPreviewOwner>();

/**
 * Meld en levende preview inn. Returverdien melder den ut igjen, og er ment
 * som `useEffect`-opprydding.
 */
export function registerCameraPreview(owner: CameraPreviewOwner): () => void {
  owners.add(owner);
  return () => {
    owners.delete(owner);
  };
}

/**
 * Slipp kameraet, nå.
 *
 * Kopien av settet er ikke pynt: en eier som melder seg ut mens vi går gjennom
 * registeret (en `stop` som fører til en avmontering) ville ellers endret
 * samlingen under føttene på løkka.
 */
export function releaseCameraPreview(): void {
  for (const owner of [...owners]) owner.stop();
}

/** Angre slippet — se filhodet: det ble ikke noe opptak likevel. */
export function resumeCameraPreview(): void {
  for (const owner of [...owners]) owner.resume();
}

/** Hvor mange levende previews registeret kjenner. Kun for tester. */
export function cameraPreviewCount(): number {
  return owners.size;
}
