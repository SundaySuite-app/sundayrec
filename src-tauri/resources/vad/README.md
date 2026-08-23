# Vendored voice-activity model (Etappe 9)

`silero_vad_op18_ifless.onnx` — Silero VAD, ONNX opset 18, the `_ifless` export.

|          |                                                                                                                |
| -------- | -------------------------------------------------------------------------------------------------------------- |
| Source   | <https://raw.githubusercontent.com/snakers4/silero-vad/v6.2.1/src/silero_vad/data/silero_vad_op18_ifless.onnx> |
| Upstream | <https://github.com/snakers4/silero-vad> (tag `v6.2.1`)                                                        |
| Size     | 2 845 718 bytes                                                                                                |
| SHA-256  | `7671cd04b004e9076da0d4a7b1a5aec36adf161c39230c1cb94a4fd5db6bbd28`                                             |
| Licence  | MIT — Copyright (c) 2020-present Silero Team (`LICENSE-silero-vad.txt`)                                        |

## Why this exact file

The better-known `silero_vad.onnx` **does not load in `tract` at all**: its `If`
node carries a dead 8 kHz branch that fails shape inference before the condition
can be constant-folded. `silero_vad_16k_op15.onnx` fails too — its branch outputs
differ in rank. The `_ifless` export has the branch already resolved away and is
the only one of the three that works. Do not "simplify" the filename.

## Why it is vendored rather than downloaded

The whisper models this app used to ship (transcription left in v0.15) were
fetched at run time with a pinned SHA, and the E9 plan originally copied that.
Whisper's models were 148 MB – 1.5 GB, so keeping them out of the installer
was worth a download path. This one is 2.8 MB.
Downloading it would buy a download UI, progress reporting, failure states, a
cache location and a "model missing" branch in the analysis path — five new ways
for analysis to fail — in exchange for 2.8 MB. Vendored, it is simply always
there, and analysis works on a church PC with no internet.

The MIT licence permits redistribution; the licence text ships beside the file.

## Integrity

The SHA-256 above is asserted **twice**, because a wrong model does not crash a
VAD — it returns confident, wrong numbers:

1. **Build time** — `src-tauri/build.rs`, whenever `--features vad` is on.
   Hashes the file on disk and fails the build on mismatch.
2. **Load time** — `sundayrec::vad::verify_embedded_model`, before the graph is
   parsed. Hashes the bytes actually linked in by `include_bytes!`.

The digest and filename live in `sundayrec_core::vad` as
`VAD_MODEL_SHA256` / `VAD_MODEL_FILE_NAME`; `build.rs` duplicates them because a
build script cannot depend on a workspace member, and
`build_script_pins_the_same_digest_as_the_core` fails if the two ever drift.

## Replacing it

Fetch the new file, then update **all** of:

- this README's table,
- `VAD_MODEL_SHA256` / `VAD_MODEL_LEN` / `VAD_MODEL_SOURCE_URL` in
  `crates/sundayrec-core/src/vad.rs`,
- the `SHA256` literal in `src-tauri/build.rs`.

Then re-run `cargo test --features vad --lib vad::`. Expect `slots_come_from_the_
graphs_own_names` to need attention: Silero's exports disagree on input order.
