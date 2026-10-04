import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import {
  bitrateKbps,
  estimatedBytes,
  exportErrorKey,
  exportKbps,
  exportStem,
  folderLabel,
  folderOf,
  isCancelled,
  isIsoDate,
  localIsoDate,
  megabytes,
  parseSavedContent,
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

  it("med tittel får fila tittelen, med dato foran", () => {
    expect(
      predictedOutputName(
        "/Opptak/gudstjeneste_2026-09-27.mp3",
        "mp3",
        "Den gode hyrde",
        "2026-09-27",
      ),
    ).toBe("2026-09-27 Den gode hyrde.mp3");
    // Uten dato (en fil utenfra biblioteket): bare tittelen.
    expect(
      predictedOutputName("/Opptak/import.wav", "flac", "Den gode hyrde", null),
    ).toBe("Den gode hyrde.flac");
    // En tom tittel er ingen tittel.
    expect(
      predictedOutputName("/Opptak/tale.mp3", "mp3", "   ", "2026-09-27"),
    ).toBe("tale_redigert.mp3");
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

  it("mono-avvisningen har sin egen setning — ikke den generelle", () => {
    // Sømmen stopper en reparasjon som leser høyre inngangskanal på en
    // monofil. Uten raden her får den frivillige den generelle setningen om
    // at «noe gikk galt», for den ene feilen som har et konkret svar.
    expect(exportErrorKey("validation: channel_repair_needs_stereo")).toBe(
      "errChannelRepairNeedsStereo",
    );
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

  // F2-A-B: bakendens enkelt-flyt-vakt (`ExportEngine::try_begin`) avviser en
  // andre eksport mens den første går. Rust-siden pinner den samme strengen i
  // `the_busy_refusal_uses_the_code_the_renderer_translates` — to sider av én
  // skjøt, hver med sin egen test på seg.
  it("export_already_running har en setning, ikke en råstreng fra en annen prosess", () => {
    expect(exportErrorKey("validation: export_already_running")).toBe(
      "errExportAlreadyRunning",
    );
  });

  // F2-11: diskvakten FØR renderen. Rust-siden pinner den samme ledende koden
  // i `the_low_disk_refusal_uses_the_code_the_renderer_translates`.
  it("disk_low_for_export har sin egen setning — ikke disk_full sin", () => {
    expect(
      exportErrorKey(
        "recording error: disk_low_for_export: 120 MB free, ~980 MB needed",
      ),
    ).toBe("errDiskLowForExport");
    // De to er ikke det samme: den ene kommer før ventetiden, den andre er
    // ffmpeg som gikk tom midtveis.
    expect(exportErrorKey("recording error: disk_low_for_export")).not.toBe(
      "errDiskFull",
    );
  });

  // A2: mappen fra «Velg mappe …» er en lapp Rust slår opp. Rust-siden pinner
  // de samme kodene mot denne tabellen i
  // `every_export_folder_refusal_has_a_sentence_in_the_renderer`.
  it("mappelappen har tre setninger: velg på nytt, borte, avvist", () => {
    expect(
      exportErrorKey(
        "validation: export_folder_unknown: this session has no export folder by that token",
      ),
    ).toBe("errExportFolderPickAgain");
    // Velgeren som lukket seg uten svar betyr det samme for den som sitter der.
    expect(
      exportErrorKey(
        "internal: dialog_failed: the dialog closed without answering",
      ),
    ).toBe("errExportFolderPickAgain");
    expect(
      exportErrorKey(
        "validation: export_folder_missing: the chosen folder is no longer there",
      ),
    ).toBe("errExportFolderMissing");
    expect(
      exportErrorKey(
        "validation: export_folder_refused: that folder cannot take an export",
      ),
    ).toBe("errExportFolderRefused");
  });

  // A2 (PR-C): opptaket og jinglene er lapper/innstillinger, ikke stier. Rust
  // pinner kodene mot tabellen i `every_source_refusal_has_a_sentence_in_the_renderer`.
  it("opptakslappen har setninger: åpne på nytt, borte, avvist — og jingelen", () => {
    expect(
      exportErrorKey(
        "validation: source_unknown: this session has no recording by that token",
      ),
    ).toBe("errSourceUnknown");
    expect(
      exportErrorKey(
        "validation: source_missing: the recording is no longer there",
      ),
    ).toBe("errFileNotFound");
    expect(
      exportErrorKey(
        "validation: source_refused: that file cannot be opened in the editor",
      ),
    ).toBe("errSourceRefused");
    expect(
      exportErrorKey(
        "validation: export_clip_unusable: the saved intro or outro clip cannot be used",
      ),
    ).toBe("errExportClipUnusable");
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

describe("filnavnet speiler kjernen", () => {
  // Den SAMME fila `export_stem_matches_the_shared_vectors` leser i Rust. Går
  // denne rød og ikke den, har forhåndsvisningen begynt å love et annet navn
  // enn det bakenden skriver.
  const vectors = JSON.parse(
    readFileSync(
      join(
        import.meta.dirname,
        "../../crates/sundayrec-core/tests/fixtures/export-stem.json",
      ),
      "utf8",
    ),
  ) as Array<{
    name: string;
    source: string;
    title: string | null;
    date: string | null;
    expect: string;
  }>;

  it("fixturen har vektorene sine", () => {
    expect(vectors.length).toBeGreaterThanOrEqual(10);
  });

  for (const v of vectors) {
    it(v.name, () => {
      expect(exportStem(v.source, v.title, v.date)).toBe(v.expect);
    });
  }

  it("en lang tittel kuttes på et tegn, ikke midt i det", () => {
    const stem = exportStem("x", "🙏".repeat(120), null);
    expect(Array.from(stem)).toHaveLength(100);
  });
});

describe("datoen", () => {
  it("er YYYY-MM-DD og finnes", () => {
    expect(isIsoDate("2026-09-27")).toBe(true);
    expect(isIsoDate("2028-02-29")).toBe(true);
    expect(isIsoDate("2026-02-30")).toBe(false);
    expect(isIsoDate("2026-9-7")).toBe(false);
    expect(isIsoDate("")).toBe(false);
  });

  it("regnes i lokal tid, slik opptakets eget filnavn gjør", () => {
    const ms = new Date(2026, 8, 27, 0, 30).getTime();
    expect(localIsoDate(ms)).toBe("2026-09-27");
    expect(localIsoDate(null)).toBeNull();
  });
});

describe("innholdet i sidevogna", () => {
  it("leser de tre strengene og ignorerer resten", () => {
    expect(
      parseSavedContent({
        title: "T",
        speaker: 3,
        description: "D",
        chapters: [{ time: 0, title: "x" }],
      }),
    ).toEqual({ title: "T", speaker: "", description: "D" });
  });

  it("en tom eller ukjent sidevogn er ingenting", () => {
    expect(parseSavedContent(null)).toBeNull();
    expect(parseSavedContent("tull")).toBeNull();
    expect(parseSavedContent({ title: "", speaker: "" })).toBeNull();
  });
});
