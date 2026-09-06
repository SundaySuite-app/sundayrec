/**
 * Bildestørrelsen, lest ut av JPEG-ens EGEN header.
 *
 * ## Hvorfor ikke bare la bildet finne den selv
 *
 * Overleggets kamerabilde er en `<img>` som får en ny `data:`-URL ~12 ganger i
 * sekundet. Uten en kjent størrelse har rammen ingen høyde før det FØRSTE
 * bildet er dekodet, og flaten hopper — midt i en gudstjeneste, på den ene
 * skjermen ingen skal trenge å se på. Headeren gir svaret på de første ~300
 * bytene, altså før bildet er lastet, og rammen reserverer plassen med
 * `--rec-video-ar` (`aspect-ratio`).
 *
 * ## Ett svar, ikke ett per frame
 *
 * Kalleren spør ÉN gang (`PolledCameraPreview` husker at den har svaret).
 * Kameraet bytter ikke oppløsning midt i et opptak, og en parsing per frame
 * ville vært 12 base64-dekodinger i sekundet på hovedtråden for et svar som
 * ikke endrer seg.
 *
 * ## Skanningen
 *
 * Portert fra det utsendte skallet (`d982012`s `pages/recording.ts:54`), med
 * én tilføyelse: den STOPPER ved `SOS` (`ff da`). Etter starten på skanningen
 * er alt entropikodet data der `ff 00`-fyllbytes ser ut som segmenter med
 * tilfeldig lengde, og den gamle løkka gikk videre inn i den suppa. Både
 * `SOF0` (baseline) og `SOF2` (progressiv) kommer FØR `SOS` i enhver gyldig
 * JPEG, så stoppet kan ikke kaste bort et svar — det kan bare la være å finne
 * et som ikke finnes.
 */

/** Bredde og høyde i piksler. */
export interface JpegDims {
  w: number;
  h: number;
}

/** Start-of-frame: baseline og progressiv. Begge bærer størrelsen likt. */
const SOF0 = 0xc0;
const SOF2 = 0xc2;
/** Start of scan — herfra og ut er det entropikodet data, ikke segmenter. */
const SOS = 0xda;
/** Markører uten lengdefelt: SOI, EOI, TEM og de åtte restart-markørene. */
const STANDALONE = new Set([0xd8, 0xd9, 0x01]);

/**
 * Størrelsen, eller `null` hvis den ikke står i det vi fikk se.
 *
 * `null` er et ærlig svar og ikke en feil: kalleren har bare de første ~1 kB
 * av fila, og en JPEG med et uvanlig stort ICC-profil-segment foran kan ha
 * headeren sin lenger inn. Da beholder rammen sitt standard-sideforhold.
 */
export function readJpegDims(arr: Uint8Array): JpegDims | null {
  let i = 0;
  while (i < arr.length - 8) {
    if (arr[i] !== 0xff) {
      i++;
      continue;
    }
    const marker = arr[i + 1];
    // `ff ff` er lovlig fyll foran en markør — gå ett hakk, ikke to.
    if (marker === 0xff) {
      i++;
      continue;
    }
    if (marker === SOF0 || marker === SOF2) {
      const h = (arr[i + 5] << 8) | arr[i + 6];
      const w = (arr[i + 7] << 8) | arr[i + 8];
      if (w > 0 && h > 0) return { w, h };
    }
    if (marker === SOS) return null;
    if (!STANDALONE.has(marker) && !(marker >= 0xd0 && marker <= 0xd7)) {
      const segment = (arr[i + 2] << 8) | arr[i + 3];
      if (segment >= 2) {
        i += 2 + segment;
        continue;
      }
    }
    i += 2;
  }
  return null;
}

/**
 * Hvor mye av base64-strengen som dekodes før headeren letes fram.
 *
 * Legacys tall, og det MÅ være delelig på fire: `atob` på en base64-streng
 * kuttet midt i en firergruppe kaster. 1400 tegn er 1050 byte, som er godt
 * forbi enhver realistisk JFIF/Huffman-header og langt fra en hel frame.
 */
export const HEADER_B64_CHARS = 1400;

/**
 * Samme svar, fra bakendens base64-frame.
 *
 * Skilt fra `readJpegDims` fordi dekodingen er det eneste her som kan kaste:
 * en avkortet eller ødelagt streng skal gi `null`, ikke ta ned overlegget midt
 * i et opptak.
 */
export function jpegDimsFromBase64(b64: string): JpegDims | null {
  try {
    const head = b64.slice(0, HEADER_B64_CHARS);
    const binary = atob(head);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return readJpegDims(bytes);
  } catch {
    return null;
  }
}
