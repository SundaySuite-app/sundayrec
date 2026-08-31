# Konsollfunn under fotograferingen (Fase A)

En streng ny vakt over en gammel app. Hver scene i atlaset kjøres med en lytter
på `console.error` og `pageerror`; alt som falt ut står her.

**Ingenting er fikset.** Dette er en observasjon, ikke en oppgave — Fase A rører
ikke appen.

## Slik er funnene klassifisert

Atlaset kjører **uten backend**. `api-shim` fanger hvert avviste `invoke` og
returnerer kallerens fallback, så «ingen backend» er den sanne tilstanden, ikke
en feil. Meldinger som matcher `no Tauri backend in the browser tier` (og
`asset://`-URL-er som aldri kan lastes i en nettleser) er derfor ført opp som
**harness-støy**. Alt annet er ekte konsollgjeld appen bærer i dag.

Grensetilfeller er ført opp som **ekte funn**, ikke som støy: en melding som
ikke kan klassifiseres med sikkerhet skal være synlig.

Merk også at appens egne `console.warn` for hvert fallback IKKE fanges her —
vakten ser bare `console.error` og ufangede unntak. Warn-strømmen er stor og
forventet uten backend.

## Ekte funn

Les hver rad sammen med scenens oppskrift i [INDEX.md](INDEX.md): noen scener
**fyrer en feil med vilje** (`home--backend-feil`, `home--kvalitetsalarm`,
`editor--feil`, `toast--lagring-feilet`). En `console.error` derfra er
riktig oppførsel, ikke gjeld.

| Scene | Språk | Type | Melding |
| --- | --- | --- | --- |
| `home--kvalitetsalarm` | no | console.error | [recording] QUALITY ALARM: [input_overflow, device_reopened] 3120/5400s |
| `home--kvalitetsalarm` | en | console.error | [recording] QUALITY ALARM: [input_overflow, device_reopened] 3120/5400s |

## Harness-støy (forventet, ikke gjeld)

| Antall | Melding |
| --- | --- |
| 17 | Failed to load resource: net::ERR_UNKNOWN_URL_SCHEME |

---

Scener fotografert: 137. Ekte funn: 2. Støy-treff: 17.
