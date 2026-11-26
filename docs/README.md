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

| Dokument                             | Hva                                                                                              |
| ------------------------------------ | ------------------------------------------------------------------------------------------------ |
| [PLAN.md](PLAN.md)                   | Hva SundayRec er nå, hva «rent bord» bevisst fjernet, og alt som gjenstår (rigg og eier)         |
| [VARSLING.md](VARSLING.md)           | Hvordan appen sier fra når noe går galt — runder, status og det som gjenstår                     |
| [RIG-DAY.md](RIG-DAY.md)             | Riggdagen: én økt som beviser riggpunktene, i rekkefølge                                         |
| [SMOKE-TEST.md](SMOKE-TEST.md)       | Maskinvare-runbook for kjernen (Ta opp, Opptak, Plan, Innstillinger) — hva som skal skje på rigg |
| [AVHENGIGHETER.md](AVHENGIGHETER.md) | Kode og biblioteker utenfra: hva lisensen tillater, og hva som står på lista                     |
| [design/](design/)                   | Designunderlag og atlas-skjermbilder                                                             |

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
| [PRO-AUDIO-WINDOWS.md](PRO-AUDIO-WINDOWS.md) | Lyd på Windows (WASAPI/ASIO) forklart for den som setter opp |

Bruksanvisningen for frivillige ([FRIVILLIG.md](archive/FRIVILLIG.md)) er arkivert: den beskrev
funksjoner som er fjernet i «rent bord», og skrives på nytt når kjernen er rigg-testet.

## Referanse — hvordan ting virker

| Dokument                                                 | Hva                                                                    |
| -------------------------------------------------------- | ---------------------------------------------------------------------- |
| [ASIO-TEST-MATRIX.md](ASIO-TEST-MATRIX.md)               | Windows-lyd: test- og utgivelsessjekkliste for WASAPI/ASIO             |
| [BUILD_ASIO.md](BUILD_ASIO.md)                           | Bygge med ASIO-støtte på Windows                                       |
| [WINDOWS-PROCESS-HYGIENE.md](WINDOWS-PROCESS-HYGIENE.md) | Hvorfor lydtjenesten krasjet på kirke-PC-en, og hva appen gjør med det |

## Historikk

[archive/](archive/) har det som ikke lenger er sant, med en linje øverst om hvorfor:
planene og øktloggene for ombyggingen
([OMBYGGING](archive/OMBYGGING.md), [GJENNOMFORING](archive/GJENNOMFORING.md),
[LYDMOTOR](archive/LYDMOTOR.md), [VIDEOMOTOR](archive/VIDEOMOTOR.md)),
[MIGRATION-TAURI2](archive/MIGRATION-TAURI2.md), [TYPESCRIPT-7](archive/TYPESCRIPT-7.md),
[APP-SHELL](archive/APP-SHELL.md) (frontend-skallet før «rent bord»),
[NEEDS-RICHARD](archive/NEEDS-RICHARD.md), [SOMMER-2026-11](archive/SOMMER-2026-11.md),
[FRIVILLIG](archive/FRIVILLIG.md), og eldre revisjoner og øktrapporter
(`BACKLOG-AUDIT-2026-07-07`, `NATT-*`, `COMMAND_AUDIT_2026-08`, `COMPLETION`,
`EDITOR-PORT`, `PARITY-BACKEND`, `RELEASE-AUDIT-2026-06-01` m.fl.).
