import { describe, expect, it } from "vitest";

import {
  bitrateKbps,
  estimatedBytes,
  exportErrorKey,
  exportKbps,
  folderLabel,
  folderOf,
  isCancelled,
  megabytes,
  predictedOutputName,
} from "./export-core";

describe("bitraten", () => {
  it("kommer fra kvalitetsvalget i Oppsett", () => {
    expect(bitrateKbps("192")).toBe(192);
    expect(bitrateKbps(320)).toBe(320);
  });

  it("faller tilbake på 256 og aldri på 0", () => {
    // Samme regel som `app/state/disk.ts`: en tom eller ugyldig verdi er ikke
    // «null kilobit», den er «vi vet ikke, bruk standarden».
    expect(bitrateKbps("")).toBe(256);
    expect(bitrateKbps(null)).toBe(256);
    expect(bitrateKbps("tull")).toBe(256);
    expect(bitrateKbps(0)).toBe(256);
  });
});

describe("kilobit per sekund", () => {
  const stereo48 = { channels: 2, sampleRate: 48_000 };

  it("mp3 bruker bitraten som er valgt", () => {
    expect(exportKbps("mp3", stereo48, 192)).toBe(192);
  });

  it("wav regnes ut av FILAS rate og kanaler, ikke av innstillingene", () => {
    // Nøyaktig poenget: et 96 kHz-opptak eksportert til WAV er dobbelt så stort
    // som opptaksinnstillingens 48 kHz ville anslått.
    expect(exportKbps("wav", stereo48, 256)).toBe(1536);
    expect(exportKbps("wav", { channels: 2, sampleRate: 96_000 }, 256)).toBe(
      3072,
    );
    expect(exportKbps("wav", { channels: 1, sampleRate: 48_000 }, 256)).toBe(
      768,
    );
  });

  it("en ukjent rate anslås som 48 kHz stereo", () => {
    expect(exportKbps("wav", { channels: null, sampleRate: null }, 256)).toBe(
      1536,
    );
  });

  it("flac er legacys eget anslag", () => {
    expect(exportKbps("flac", stereo48, 256)).toBe(600);
    expect(exportKbps("flac", { channels: 1, sampleRate: 48_000 }, 256)).toBe(
      350,
    );
  });
});

describe("størrelsesanslaget", () => {
  it("er kbps · 125 · sekunder — samme regnestykke som diskanslaget", () => {
    // 28 min 10 s tale i 256 kbps ≈ 54 MB.
    expect(estimatedBytes(1690, 256)).toBe(54_080_000);
  });

  it("svarer ingenting når det ikke er noe å regne på", () => {
    expect(estimatedBytes(0, 256)).toBeNull();
    expect(estimatedBytes(600, 0)).toBeNull();
    expect(estimatedBytes(Number.NaN, 256)).toBeNull();
  });

  it("megabyte får én desimal under ti og ingen over", () => {
    expect(megabytes(27_400_000)).toBe(27);
    expect(megabytes(2_340_000)).toBe(2.3);
    expect(megabytes(null)).toBeNull();
    expect(megabytes(0)).toBeNull();
  });
});

describe("navnet og mappen", () => {
  it("forutsier bakendens `<navn>_redigert.<ext>`", () => {
    expect(
      predictedOutputName("/Opptak/2026-08-23 Gudstjeneste.mp3", "mp3"),
    ).toBe("2026-08-23 Gudstjeneste_redigert.mp3");
    // Formatet kan være et annet enn kildens.
    expect(predictedOutputName("/Opptak/tale.wav", "flac")).toBe(
      "tale_redigert.flac",
    );
    // En fil uten endelse mister ikke navnet sitt.
    expect(predictedOutputName("/Opptak/tale", "mp3")).toBe(
      "tale_redigert.mp3",
    );
  });

  it("«Samme mappe» er opptakets egen mappe", () => {
    expect(folderOf("/Users/a/Opptak/b.mp3")).toBe("/Users/a/Opptak");
    expect(folderOf("C:\\Opptak\\b.mp3")).toBe("C:\\Opptak");
    expect(folderOf("b.mp3")).toBe("");
  });

  it("mappenavnet er det siste leddet — det brukeren kjenner igjen", () => {
    expect(folderLabel("/Users/a/Documents/SundayRec")).toBe("SundayRec");
    expect(folderLabel("/Users/a/Opptak/")).toBe("Opptak");
    expect(folderLabel("")).toBe("");
  });
});

describe("feilkodene", () => {
  it("kjenner igjen den ledende koden, ikke prosa som nevner den", () => {
    expect(exportErrorKey("validation: no_audio_remaining")).toBe(
      "errNoAudioRemaining",
    );
    expect(exportErrorKey("recording error: timeout: after 900s")).toBe(
      "errTimeout",
    );
    expect(exportErrorKey("not found: file_not_found")).toBe("errFileNotFound");
    expect(exportErrorKey("validation: invalid_duration")).toBe("errCutData");
  });

  it("path_guard-meldingen matcher fortsatt på innhold — den har ingen kode", () => {
    expect(exportErrorKey("path must be absolute: ../ut")).toBe(
      "errPathNotAbsolute",
    );
  });

  // F2-A-A: `path_guard::checked_input_file` sier «cannot resolve path …»
  // FØR `export()` selv rekker å si `file_not_found` — uten denne rada var
  // den vanligste måten en kildefil forsvinner på (frakoblet disk, flyttet
  // eller slettet fil) usynlig for tabellen, og «er disken frakoblet?»-
  // setningen ble aldri vist for nettopp det spørsmålet.
  it("«cannot resolve path» fra path_guard gir samme setning som file_not_found", () => {
    expect(
      exportErrorKey(
        "validation: cannot resolve path /Opptak/borte.mp3: No such file or directory (os error 2)",
      ),
    ).toBe("errFileNotFound");
  });

  // F2-A-A: en full disk klassifiseres i Rust (samme mønster som
  // opptakeren) og krysser IPC med `disk_full` som den ledende koden.
  it("disk_full er en egen, ledende kode", () => {
    expect(
      exportErrorKey(
        "recording error: disk_full: av_interleaved_write_frame(): No space left on device",
      ),
    ).toBe("errDiskFull");
  });

  it("en ukjent kode gir ingenting, ikke en råstreng", () => {
    expect(exportErrorKey("internal: noe_helt_nytt")).toBeNull();
    expect(exportErrorKey(undefined)).toBeNull();
  });

  // F2-A-A: den tidligere fallbacken søkte med `includes` over HELE
  // meldingen for alle sju kodene, ikke bare de flerords-fraser som aldri
  // kan bli `lead` — så et ord som «timeout» eller «cancelled» i 500 tegn rå
  // ffmpeg-prosa (helt urelatert til vår egen `timeout`/`cancelled`-kode) ga
  // feil setning. Den er nå strammet til KUN å gjelde koder med mellomrom.
  it("et ord som nevner en kjent kode i fri ffmpeg-prosa gir ingen treff", () => {
    expect(
      exportErrorKey(
        "recording error: ffmpeg failed: Connection timeout while probing filter graph",
      ),
    ).toBeNull();
    expect(
      exportErrorKey(
        "recording error: ffmpeg failed: operation cancelled by remote peer",
      ),
    ).toBeNull();
  });

  it("avbrutt er ikke en feil", () => {
    expect(isCancelled("recording error: cancelled")).toBe(true);
    expect(isCancelled("validation: timeout")).toBe(false);
    expect(isCancelled(undefined)).toBe(false);
    // Prosa som bare nevner ordet må heller ikke leses som en avbryting.
    expect(
      isCancelled(
        "recording error: ffmpeg failed: operation cancelled by remote peer",
      ),
    ).toBe(false);
  });
});
