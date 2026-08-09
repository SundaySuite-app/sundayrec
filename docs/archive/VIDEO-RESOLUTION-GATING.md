# Video-oppløsning: gating mot kilden (ingen oppskalering)

**Prinsipp (Richard):** Det er greit å **down-scale** (ta opp lavere enn kameraet
leverer), men aldri **up-scale** — vi skal aldri ta opp i en høyere oppløsning enn
det kilden faktisk gir native. Hvis vi ikke kan _bekrefte_ at kilden leverer ekte
4K, skal 4K ikke tilbys.

## Hvordan det fungerer nå

1. **Probe:** ved kamera-valg probes kameraets _annonserte_ moduser
   (`probe_camera_modes` → avfoundation/dshow «Supported modes»).
2. **Oppsummering** (`summarize_camera_capabilities`, core, enhetstestet): en
   16:9-tag (480p/720p/1080p/2160p) tilbys KUN hvis en modus er minst like stor i
   **begge** dimensjoner (kan croppe/skalere NED til den). Ikke bare høyde — så en
   kvadrat-/portrett-modus ikke kan «låse opp» en 16:9-tag den ikke kan fylle.
3. **UI-gating** (`applyCameraCapabilities`, video-siden): oppløsninger over
   kameraets tak gråes ut (`is-disabled` + «ikke støttet»-badge), kameraets maks
   får «kameraets maks»-badge, og et allerede valgt for-høyt valg faller ned til
   maks.
4. **Recorder** (`resolve_camera_mode`): pinner `-video_size` til den nærmeste
   EKTE modusen (aspekt-ratio først), så selv om en for-høy verdi snek seg
   gjennom, tas det opp i en reell modus — aldri en oppskalert/forvrengt.

## Situasjoner hvor kilden ikke har oppløsningen

| Situasjon                                                                                                        | Hva skjer / hvordan håndtert                                                                                                                                                                                                                                                      |
| ---------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **1080p-kamera, 4K valgt** (FaceTime HD)                                                                         | Kameraet annonserer maks 1920×1080 (16:9). 2160p gråes ut. Recorder ville uansett pinne 1920×1080. ✅                                                                                                                                                                             |
| **Kamera med høyoppløst KVADRAT-modus** (FaceTime 1552×1552)                                                     | Kvadraten har flere piksler enn 1080p, men fyller ikke en 16:9-4K-ramme → 2160p forblir gated (både-dimensjoner-sjekken). Recorderen velger 16:9 (1920×1080), ikke den zoomede kvadraten. ✅ (var den opprinnelige «4K er zoomet»-buggen)                                         |
| **Billig webkamera, maks 720p**                                                                                  | Kun 480p/720p tilbys; 1080p/4K gråes ut. ✅                                                                                                                                                                                                                                       |
| **Kamera «ikke skrudd på» i 4K / står i 1080p-modus**                                                            | Vi går på det kameraet ANNONSERER. Annonserer det bare ≤1080p, gråes 4K ut — vi kan ikke tvinge en modus kameraet ikke eksponerer. Hvis det annonserer 4K, tilbys det. ✅                                                                                                         |
| **4:3-/portrett-kamera**                                                                                         | 16:9-tags tilbys kun hvis en modus er stor nok i begge dim. Recorderen foretrekker aspekt-match; et 4:3-kilde croppes/skaleres til nærmeste, aldri oppskalert. ✅                                                                                                                 |
| **Kun lav fps tilgjengelig** (1080p\@15)                                                                         | fps-valg gråes til ≤ kameraets maks-fps; for-høy fps faller ned. ✅                                                                                                                                                                                                               |
| **Probe FEILER / kamera opptatt** (live-preview holder kameraet, virtuelt kamera, capture-kort uten modus-liste) | **Konservativt:** i stedet for å tilby alt, antar vi maks **1080p** og gråer ut 4K, med tydelig melding: «Kunne ikke lese kameraets oppløsninger — begrenset til 1080p for å unngå oppskalering.» Down-scaling er fortsatt fritt. ✅ (var tidligere «tilby alt» = upscale-fellen) |

## Bevisste valg / avveininger

- **Probe-feil → 1080p-tak:** et ekte 4K-kamera med en mislykket probe blir
  midlertidig begrenset til 1080p. Det er den trygge siden av Richards regel
  (ikke oppskaler uten bekreftelse). Oppfølging om ønskelig: en «jeg vet kameraet
  er 4K — lås opp»-override.
- **Vi forcer ikke kameraet inn i en modus** det ikke annonserer (f.eks. å «skru
  på» 4K på et kamera som står i 1080p). Vi speiler kun det avfoundation/dshow
  rapporterer.
- Recorderen har siste ord: den pinner alltid en reell annonsert modus, så feil i
  gatingen kan i verste fall gi feil _tilbud_, men aldri et ødelagt/oppskalert
  opptak.

## Testdekning (core, enhetstestet)

- `capabilities_gate_resolution_and_fps_to_advertised` — 1080p-kamera ⇒ ≤1080p.
- `capabilities_square_mode_does_not_unlock_4k` — kvadrat-modus låser ikke 4K.
- `capabilities_4k60_camera_supports_everything` — ekte 4K ⇒ alt tilbys.
- `resolve_4k_target_picks_16_9_not_a_square_mode` — recorder velger 16:9, ikke kvadrat.
