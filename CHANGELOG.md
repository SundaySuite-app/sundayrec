# Endringslogg

Merkbare endringer for deg som bruker SundayRec. Eldre utgivelser enn v0.9.0 er
dokumentert i [utgivelsene på GitHub](https://github.com/SundaySuite-app/sundayrec/releases).

## v0.9.0 — UX-gjennomgang

Denne versjonen retter seg mot den frivillige som skrur på opptaket søndag
morgen og ikke skal måtte lure på om det gikk bra. Ingenting av lydmotoren fra
v0.8.1 er rørt — det er grensesnittet, tilbakemeldingene og ærligheten i det du
ser som er bygget om.

### Du får vite hva som skjer

- **Glipp-varsel.** Hvis et planlagt opptak ikke ble til noe, sier appen det med
  én gang du åpner den — med hvilke opptak det gjaldt, og hvorfor det pleier å
  skje. Før måtte du oppdage det selv i Historikk.
- **Sjekk før start.** Et kort på Hjem forteller om noe står i veien for
  neste planlagte opptak (lydenhet borte, for lite lagringsplass, manglende
  tillatelse til mikrofon eller kamera) mens det ennå er tid til å ordne det.
- **Én sannhet om «neste opptak».** Fem steder i appen regnet ut hvert sitt svar
  på når neste opptak var, og de var ikke alltid enige. Nå er det ett svar.
- **«Fullfører opptak…».** Når du stopper, sier appen at den skriver ferdig fila
  i stedet for å se ut som om ingenting skjer.
- **Tap av data får sitt eget varsel** som blir stående til du har lest det, i
  stedet for å kaste deg til en annen side.
- **Menylinje-ikonet lever.** Ikonet viser om det spilles inn, menyen viser
  gjeldende status, og den følger språkvalget ditt.

### Færre steder å gå feil

- **Fem faner i Innstillinger i stedet for sju**: Lyd · Video · Opptak · Deling
  · System. «Publisering» og «Varsler» var to halvdeler av samme spørsmål — hvem
  som får opptaket etterpå — og er nå seksjoner under Deling.
- **Innstillinger lagres når du endrer dem**, med en «Lagret ✓»-kvittering. Den
  gamle «Lagre»-foten er borte — sammen med en feil der Lagre-knappen på
  Video-siden aldri kunne nås.
- **Ærlige flater.** Funksjoner uten noe bak seg i denne bygningen sier det rett
  ut («Kommer», «Ikke tilgjengelig», «Ikke konfigurert») og er slått av, i
  stedet for å ta imot klikk som ikke gjør noe.
- **Onboarding** har fått tilbake-knapp, og tidsplanen du setter opp der legges
  til i stedet for å overskrive det som allerede fantes.

### Redigering og historikk

- **Historikk** kan sorteres og filtreres, sletting spør først, og lyd- og
  videofil fra samme opptak paret aldri i Tauri-versjonen — det gjør de nå.
- **Preken-søket virker igjen**, sammen med gjennomgangskøen og eksport av
  transkripsjon (også som ren tekst).
- **Editoren** har fått en fastlåst lagre-linje som ikke forsvinner når du
  scroller, synlig resultat etter eksport, og hurtigtastene bak et «?».
- **Roligere bilde.** Zoom med hjulet og avspilling tegner én gang per bilde i
  stedet for å kjempe mot seg selv.

### Detaljene

- Alle systemdialoger («Er du sikker?») er erstattet med appens egne — de kan
  oversettes, de kan avbrytes med Escape, og de fryser ikke vinduet.
- Nivåmålerne oppfører seg likt på 30, 60 og 120 bilder i sekundet.
- Gullknappene har mørk tekst (var hvitt på gull — for lite kontrast),
  fokusringer er synlige, og sidebytte tilbakestiller rullingen.
- Automatisk stopp styres nå av motoren, ikke av grensesnittet.
- **Forhåndsopptak** («preroll») ligger bak en av-som-standard bryter merket
  eksperimentell. Ikke slå den på før du har testet den på riggen din.

### Språk

Sju språk holdes i takt. Et par tekster pekte fortsatt på faner som ikke finnes
lenger, og «Maks»-avlesningen under nivåmålerne var norsk uansett språk — begge
deler er rettet.
