# UPSTREAM — cap-camera-mediafoundation

Tatt inn fra **Cap** (<https://github.com/CapSoftware/Cap>), `crates/camera-mediafoundation/`,
pakke C1b i ombyggingen ([docs/GJENNOMFORING.md](../../docs/GJENNOMFORING.md) §C1b,
[docs/VIDEOMOTOR.md](../../docs/VIDEOMOTOR.md) §Kildene).

|                                       |                                                                                                                                                                                                                                                                                                                                  |
| ------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Commit                                | `8a1480262db2b54629806e936226ff06fbced9f1` (Cap `main`, 2026-10-13)                                                                                                                                                                                                                                                              |
| Siste endring i denne mappa oppstrøms | `e370c259f9564cd44050f7d378075d86963c5818` (2026-09-08, «fix: stop repeated Windows camera resource allocation»)                                                                                                                                                                                                                 |
| Lisens                                | **MIT** — `LICENSE-MIT` her er Caps `licenses/LICENSE-MIT` uendret. Caps rot-`LICENSE` sier at «all code residing in the `cap-camera*` … crates» er MIT; resten av Cap er AGPL. `Cargo.toml` oppstrøms sier `license = "MIT"`. Ingen fil i mappa har en annen lisensmerknad (sjekket med grep etter copyright/license/GPL/SPDX). |
| Opphavsrett                           | Copyright (c) 2023 Cap Software, Inc.                                                                                                                                                                                                                                                                                            |

**Valg av commit.** Gjennomgangen 2026-10-11 brukte
`a32e4806b3004071effe00243e1dabd52bc7068c`. Nyeste `main` ved innhentingen var
`8a1480262db2`; `git log a32e4806..8a1480262db2 -- crates/camera-windows
crates/camera-mediafoundation crates/camera-directshow licenses` er tom, så
diffen mellom de to er null for disse mappene. Den nyeste er skrevet her.

**Slik ble den hentet.** `git clone --filter=blob:none --no-checkout` og
`git sparse-checkout set crates/camera-windows crates/camera-mediafoundation
crates/camera-directshow licenses`. Ingen andre crates fra Cap ble sjekket ut,
og ingen byggeskript fra klonen ble kjørt.

**AGPL-grensen.** Cap-oppstrøms avhenger av `cap-mediafoundation-utils`, som er
**AGPL**. Den er **ikke hentet, ikke lest og ikke kopiert**. Det disse cratene
brukte fra den (`IMFMediaBufferExt::lock()` og vakten `IMFMediaBufferLock`) er
skrevet på nytt i [`crates/mf-util`](../mf-util/src/lib.rs) (`sundayrec-mf-util`,
MIT) fra Microsofts dokumentasjon på learn.microsoft.com — lenkene står i
fila. Grensesnittet ble utledet fra MIT-sidene alene: kompilatorfeilene etter at
importen var byttet ut (`lock` på `IMFMediaBuffer`), bruken i
`camera-windows/src/lib.rs` (`IMFMediaBufferLock<'a>` som `Deref<Target = [u8]>`)
og README-en til `camera-mediafoundation` (`IMFMediaBufferExt::lock()`). MF-oppstart
og MMCSS brukes ikke av disse cratene; de står i `mf-util` for laget over (C1c/C3a).

Blob-hasher oppstrøms (for å kunne diffe senere):

```
36ee6fdd9d057a741ee5a6ce3fb44265054a386c Cargo.toml
5ada142f0b216b9508fb41afe4d65c389e34dd02 README.md
86e4881e37ad4ef3b084a45865af43a48446c1e9 examples/cli.rs
246b1c311e9406c35fbc73f858387268929b1996 src/lib.rs
```

## Lokale endringer

Følges ikke fortløpende oppstrøms. Ved en oppdatering: hent ny commit på samme
måte, diff mot blobene over, og før endringene inn her.

- `Cargo.toml`: som i `camera-windows` — eksplisitte versjoner, `windows`
  `=0.62.2`, `thiserror` 2, uten `workspace-hack`/`[lints]`/`inquire`,
  `cap-mediafoundation-utils` (AGPL) → `sundayrec-mf-util`, `publish = false`.
- `src/lib.rs`: `use cap_mediafoundation_utils::*;` → `use sundayrec_mf_util::*;`
  (rustfmt flyttet linja etter `use std::…`). Ellers uendret.
- `examples/cli.rs` er ikke tatt med (trenger `inquire`).
- `LICENSE-MIT` lagt til.

**Merknad, ikke endret:** `initialize_mediafoundation()` kaller
`MFStartup(MF_API_VERSION, …)`. Microsofts dokumentasjon sier `MF_VERSION`
(`MF_SDK_VERSION << 16 | MF_API_VERSION`). Vår `sundayrec_mf_util::MediaFoundation`
bruker `MF_VERSION`; C1c bør starte MF via den og ikke via denne funksjonen.
