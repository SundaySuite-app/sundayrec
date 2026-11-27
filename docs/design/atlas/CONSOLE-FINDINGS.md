# Konsollfunn under fotograferingen

Hver scene i atlaset kjøres med en lytter på `console.error` og
`pageerror`; alt som falt ut står her.

## Slik er funnene klassifisert

Atlaset kjører **uten backend**. `api-shim` fanger hvert avviste `invoke` og
returnerer kallerens fallback, så «ingen backend» er den sanne tilstanden, ikke
en feil. Meldinger som matcher `no Tauri backend in the browser tier` (og
`asset://`-URL-er som aldri kan lastes i en nettleser) er derfor ført opp som
**harness-støy**. Alt annet er ekte konsollgjeld appen bærer i dag.

Grensetilfeller er ført opp som **ekte funn**, ikke som støy: en melding som
ikke kan klassifiseres med sikkerhet skal være synlig. Lista over støymønstre er
med vilje kort og spesifikk — et bredt mønster her er den stilleste måten å
miste et ekte funn på.

Merk også at appens egne `console.warn` for hvert fallback IKKE fanges her —
vakten ser bare `console.error` og ufangede unntak. Warn-strømmen er stor og
forventet uten backend.

## Ekte funn

Les hver rad sammen med scenens oppskrift i [INDEX.md](INDEX.md): noen scener
**fyrer en feil med vilje** (feilbannere, feilede lastinger, feilede lagringer).
En `console.error` derfra er riktig oppførsel, ikke gjeld.

| Scene | Språk | Type | Melding |
| --- | --- | --- | --- |
| `tannhjul--lisenser` | no | pageerror | Failed to read the 'localStorage' property from 'Window': The document is sandboxed and lacks the 'allow-same-origin' flag. |
| `tannhjul--lisenser` | no | console.error | Blocked script execution in 'about:srcdoc' because the document's frame is sandboxed and the 'allow-scripts' permission is not set. |
| `tannhjul--lisenser` | en | pageerror | Failed to read the 'localStorage' property from 'Window': The document is sandboxed and lacks the 'allow-same-origin' flag. |
| `tannhjul--lisenser` | en | console.error | Blocked script execution in 'about:srcdoc' because the document's frame is sandboxed and the 'allow-scripts' permission is not set. |

## Harness-støy (forventet, ikke gjeld)

| Antall | Melding |
| --- | --- |
| 4 | Failed to load resource: net::ERR_UNKNOWN_URL_SCHEME |

---

Scener fotografert: 137. Ekte funn: 4. Støy-treff: 4.
