# Dokumentene i `docs/`

Kart over alt som ligger her, og hva hvert dokument er til. Hva som
**gjenstår**, står ett sted: [`PLAN.md`](PLAN.md).

**Regler**

- Et nytt dokument får en linje her i samme PR.
- Et dokument som slutter å være sant, flyttes til `archive/` med en linje
  øverst om hvorfor, og lenkene til det rettes. Det slettes ikke.
- Øktrapporter («natt-audit», «gjennomgang») hører ikke hjemme her som levende
  dokumenter: det åpne i dem føres inn i `PLAN.md`, rapporten arkiveres.

## Status og planer — levende, oppdateres når noe endres

| Dokument                             | Hva                                                                          |
| ------------------------------------ | ---------------------------------------------------------------------------- |
| [PLAN.md](PLAN.md)                   | Registeret: alt som gjenstår, hvem sitt det er, og hvor detaljene står       |
| [VARSLING.md](VARSLING.md)           | Hvordan appen sier fra når noe går galt — runder, status og det som gjenstår |
| [NEEDS-RICHARD.md](NEEDS-RICHARD.md) | Eierens punkter i detalj: kontoer, signering, beslutninger, uverifisert rigg |
| [RIG-DAY.md](RIG-DAY.md)             | Riggdagen: én økt som beviser riggpunktene, i rekkefølge                     |
| [SMOKE-TEST.md](SMOKE-TEST.md)       | Maskinvare-runbook per funksjon — hva som skal skje på en ekte maskin        |

## Utgivelse og drift

| Dokument                                     | Hva                                                                  |
| -------------------------------------------- | -------------------------------------------------------------------- |
| [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md) | Slik slippes en versjon: tagg, publiser, løft til stabil og beta     |
| [ROLLBACK.md](ROLLBACK.md)                   | Når en utgitt versjon er dårlig — hva som faktisk kan gjøres         |
| [DISTRIBUTION.md](DISTRIBUTION.md)           | Installasjonsfiler, signering, notarisering og oppdateringsstrømmen  |
| [release-notes/](release-notes/README.md)    | Releasenotatet per versjon — det brukeren ser i oppdateringsdialogen |

## For frivillige

| Dokument                                     | Hva                                                          |
| -------------------------------------------- | ------------------------------------------------------------ |
| [FRIVILLIG.md](FRIVILLIG.md)                 | Bruksanvisningen for den som tar opp gudstjenesten           |
| [PRO-AUDIO-WINDOWS.md](PRO-AUDIO-WINDOWS.md) | Lyd på Windows (WASAPI/ASIO) forklart for den som setter opp |

## Referanse — hvordan ting virker

| Dokument                                                 | Hva                                                                    |
| -------------------------------------------------------- | ---------------------------------------------------------------------- |
| [APP-SHELL.md](APP-SHELL.md)                             | Frontend-skallet i `app/`: struktur, mønstre og historikk              |
| [VAD.md](VAD.md)                                         | Stemmegjenkjenning (E9): hva som finnes, hva den får bestemme          |
| [LEARNING.md](LEARNING.md)                               | Læringssløyfa: korreksjon → lokal post → aggregat → utgivelse          |
| [ASIO-TEST-MATRIX.md](ASIO-TEST-MATRIX.md)               | Windows-lyd: test- og utgivelsessjekkliste for WASAPI/ASIO             |
| [BUILD_ASIO.md](BUILD_ASIO.md)                           | Bygge med ASIO-støtte på Windows                                       |
| [WINDOWS-PROCESS-HYGIENE.md](WINDOWS-PROCESS-HYGIENE.md) | Hvorfor lydtjenesten krasjet på kirke-PC-en, og hva appen gjør med det |
| [TYPESCRIPT-7.md](TYPESCRIPT-7.md)                       | Hvorfor `package.json` har to TypeScript-er                            |

## Historikk

| Dokument                                   | Hva                                                                   |
| ------------------------------------------ | --------------------------------------------------------------------- |
| [MIGRATION-TAURI2.md](MIGRATION-TAURI2.md) | Beslutningen om Tauri 2 + Rust (2026-05-30) og fasene — gjennomført   |
| [archive/](archive/)                       | Øktrapporter, ferdige revisjoner og seksjoner for fjernede funksjoner |

I `archive/`: `BACKLOG-AUDIT-2026-07-07` (triagert 2026-09-28, det åpne står i
`PLAN.md`), `NATT-AUDIT-2026-06-07`, `NATT-LYD-VU-PREKEN-2026-06-14`,
`NEEDS-RICHARD-historikk` (fjernede funksjoner), `COMMAND_AUDIT_2026-08`,
`COMPLETION`, `EDITOR-PORT`, `GOOGLE-OAUTH-SETUP`, `NATT-LYD-VIDEO`,
`NATT-SEAM-AUDIT`, `PARITY-BACKEND`, `PHASE6-cloud-backup`,
`RELEASE-AUDIT-2026-06-01`, `START-LATENCY-ANALYSIS`,
`VIDEO-RESOLUTION-GATING`, `WINDOWS-RIGG-START`.
