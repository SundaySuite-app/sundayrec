# Neural voice-activity detection (E9) — what exists, what it may decide, what it costs

A working note for whoever next opens `crates/sundayrec-core/src/vad.rs`,
`src-tauri/src/vad/`, `crates/sundayrec-core/src/shadow.rs` or
`crates/sundayrec-core/src/ab_eval.rs`. It records the things that are true but
not visible from the code, and the four mistakes that cost the feasibility spike
a day each because none of them produced an error.

Read this before changing a constant. Every number below is pinned by a test,
and the tests are named so you can find them.

## Status: nothing installed contains the model

| Question                                             | Answer                                                                  |
| ---------------------------------------------------- | ----------------------------------------------------------------------- |
| Is the model in the app people have?                 | **No.** Not in any release, on any platform.                            |
| Does the VAD decide anything when the feature is on? | **No.** Shadow mode: it computes, the heuristic decides.                |
| What does a shadow run leave behind?                 | One `shadowObservations` entry in `<stem>.feedback.json`. Nothing else. |

The cargo feature is `vad`, declared in `src-tauri/Cargo.toml` as
`vad = ["dep:tract-onnx", "dep:sha2"]`, and it is **not** in `default`. That
alone would not settle it — a feature can reach a release without being in
`default` — so the second half matters more:

- `.github/workflows/release.yml` builds with **explicit** feature lists, one per
  platform (`--no-default-features --features editor,tray,updater,email` on
  macOS, `editor,tray,asio,updater,email` on Windows — `streaming` left in
  v0.14, `whisper` in v0.15). Neither names `vad`.
- The repo has **no `--all-features` anywhere** — not in a workflow, not in
  `package.json`. There is no path by which the feature turns itself on.

So no installed build contains the 2.8 MB model, and no user's machine has ever
run an inference. CI does: `ci.yml` runs
`cargo clippy --workspace --all-targets --features vad -- -D warnings` and
`cargo test --workspace --features vad vad::`, which is the only place the real
graph is exercised.

And when the feature IS on — a developer build, or `--features vad` locally —
the model still decides nothing. `sundayrec_core::shadow` runs the detector a
**second** time with the model in the scorer's seat, compares that `Detection`
against the one the app already acted on, and drops it. The value the editor and
the review queue read comes from their own call with `HeuristicScorer`, which the
shadow path never touches, never wraps and cannot reach. That is stated as a
property, not a setting, in `shadow.rs`'s module header.

## The four traps

Every one of these was hit by the spike. **Not one of them produced an error.**
They produced plausible numbers that were wrong, which is the only failure mode
that matters here: a VAD fed a broken window does not crash, it returns
confident silence, and the recording it silences looks exactly like a quiet
service.

### 1. The window is 576 samples, not 512

The hop advances 512 samples, so 512 is the number in every timing calculation
and the number a future reader will "fix" this to. The tensor fed to the model is
`64 samples of the previous hop's tail ++ 512 new` = 576
(`VAD_CONTEXT_SAMPLES + VAD_HOP_SAMPLES`).

Feed a bare 512-sample window and the model returns **~0.001 for every frame of
clear speech**. No shape error, no warning — the graph loads fine, which is
exactly the problem. The carried 64 samples are the second piece of per-stream
state, and the easy one to forget because it does not look like state; it looks
like a buffer.

Guarded by, in `src-tauri/src/vad/mod.rs`:
`a_bare_512_sample_window_collapses_to_silence` (builds the wrong graph on
purpose and measures both numbers against the real model),
`the_backend_rejects_a_window_that_is_not_576`,
`dropping_the_carried_context_changes_the_answer` (proves the 64 samples are
load-bearing and not padding). In `crates/sundayrec-core/src/vad.rs`:
`window_is_context_plus_hop_not_a_bare_hop`,
`every_emitted_window_is_576_samples`,
`first_64_samples_of_each_window_are_the_previous_hops_tail`.

### 2. `sr` must be 16000

The model takes the sample rate as a **runtime input**, and tract honours it
dynamically. Feeding 8000 alongside a 16 kHz window is therefore not a shape
error; it is silently a different model. The spike measured the damage: mean
probability 0.735 vs 0.755 — which looks fine — with a **maximum per-frame
difference of 0.9986**, i.e. a frame flipped from ~0 to ~1. No error, no warning.

Hence: assert, never assume. Resampling to 16 kHz is the caller's job, and
`VadStream::new` refuses anything else. Note that `shadow::VadScorer::score_hops`
**forwards** the analysis pipeline's rate rather than substituting
`VAD_SAMPLE_RATE` — substituting the constant would make the guard unreachable
while looking like a simplification.

Guarded by: `the_sample_rate_input_actually_changes_the_output` and
`the_stream_refuses_to_be_built_at_the_wrong_rate` (src-tauri),
`stream_refuses_any_rate_but_16k` (core),
`the_wrong_sample_rate_is_refused_rather_than_scored` (shadow).

### 3. Bind inputs by name, never by index

Silero ships several ONNX exports **whose input order differs**. This file orders
them `(input, sr, state)`; the better-known `silero_vad.onnx` orders them
`(input, state, sr)`. Bind by index against the wrong one and the audio lands in
the `sr` slot — an i64 scalar, so it may or may not even error, and when it does
not, the output is garbage that still lies in 0..1.

So the slots are always **derived from the graph's own names**
(`resolve_input_slots` / `resolve_output_slots`), never written down. The same
argument applies to the outputs: reading output 0 when the state happens to be
output 0 gives 128 numbers where you expected one, and the first of them is still
a plausible probability.

Guarded by: `slots_come_from_the_graphs_own_names` (src-tauri — it also records
the current order, so a model swap that reorders inputs is visible in the diff),
`slots_follow_the_names_not_the_positions` and
`an_unknown_graph_is_refused_rather_than_guessed` (core).

### 4. The batch dimension must stay symbolic

Pinning a concrete batch dimension before `into_optimized()` fails. It has to be
left symbolic for the graph to optimize at all.

Guarded by `a_concrete_batch_dimension_does_not_survive_optimization`, which
asserts the concrete form still **errors** — so if a future tract makes it work,
the test fails and tells you to re-read this section before deleting the symbolic
form anyway.

## Why only one model file works

The vendored file is **`silero_vad_op18_ifless.onnx`**
(`src-tauri/resources/vad/`), and it is the only one of the three candidates that
loads in tract:

- **`silero_vad.onnx`** — the canonical, best-known file. Does not load at all.
  Its `If` node carries a dead 8 kHz branch that fails shape inference _before_
  the condition can be constant-folded. tract never gets to the point where it
  could see the branch is unreachable.
- **`silero_vad_16k_op15.onnx`** — fails differently: its branch outputs differ
  in rank.
- **`silero_vad_op18_ifless.onnx`** — the variant with the branch already
  resolved away. This one.

Do not "simplify" the constant to the better-known name.

| Fact     | Value                                                              |
| -------- | ------------------------------------------------------------------ |
| SHA-256  | `7671cd04b004e9076da0d4a7b1a5aec36adf161c39230c1cb94a4fd5db6bbd28` |
| Bytes    | 2 845 718                                                          |
| Upstream | silero-vad `v6.2.1`, `src/silero_vad/data/`                        |
| Licence  | MIT (Silero Team) — which is why vendoring is legal                |

The digest is asserted **twice**: `src-tauri/build.rs` hashes the file on disk
whenever the feature is on (a corrupted checkout fails the build), and
`verify_embedded_model` hashes the bytes actually linked in, before the graph is
parsed. Both are necessary for the same reason as trap 1: a wrong or truncated
model does not crash a VAD, it returns numbers.

The model is embedded with `include_bytes!` rather than bundled as a Tauri
resource, because a bundled resource has to be _found_ at run time — a
path-resolution branch that differs per platform, breaks under `cargo test`, and
fails looking like "VAD unavailable" rather than "installer is broken".

## The composition rule, and why it is not the obvious one

A voice-activity detector answers exactly one question: **is someone talking.**
It has no opinion about whether the rest is an organ, a rustle or nothing at all.
The pipeline below the scorer seam very much does.

The obvious mapping — everything the VAD rejects becomes `Silence` — is wrong,
and wrong in a way that would corrupt the measurement rather than merely degrade
it. `detect.rs` reads the **music share** twice:

- `find_sermon`'s Case 0 ("this recording is nothing but the talk") requires
  music under **5 %** of the recording (`SERMON_ONLY_MUSIC_RATIO`);
- `MOSTLY_MUSIC` — the concert warning — fires above **50 %**
  (`CONCERT_MUSIC_RATIO_THRESHOLD`).

Map non-speech to silence and every recording's music share is zero. Case 0 then
clears on services it should refuse, and `MOSTLY_MUSIC` becomes **unreachable** —
one attention reason retired without anyone deciding to retire it. Both are
changes to the ANSWER caused by the mapping, not by the model's ability to hear
speech, and they are indistinguishable from real differences in the record. The
harness would be grading a mapping.

So the model overrules exactly what it is competent to overrule:

| pooled score     | heuristic said | result                 |
| ---------------- | -------------- | ---------------------- |
| ≥ threshold      | anything       | `Speech`, conf = p     |
| < threshold      | `Speech`       | `Mixed`, conf = 1 − p  |
| < threshold      | anything else  | the heuristic's answer |
| no hops in frame | anything       | the heuristic's answer |

`Mixed` — "there is sound here, but no voice in it" — is the one verdict this
composition invents, and it is the interesting cell: it is where the two
detectors actually contradict each other.

**Both agents building on this hit the trap independently.** The shadow-mode work
and the A/B harness were written in parallel, neither could see the other, and
both arrived at the same non-obvious mapping for the same reason. That is
evidence that it is a trap and not a preference — the obvious answer is genuinely
attractive and genuinely wrong. Do not let it back in.

The rule now lives in exactly one place, `shadow::pool_onto_frames`, and
`ab_eval::PooledVadScorer` calls it rather than restating it. Guarded by
`a_rejecting_model_leaves_the_music_and_silence_classes_alone` (shadow, at the
frame level) and `non_speech_fill_changes_the_music_share` (ab_eval, through the
whole detector).

### Pooling

The grids do not line up: the model answers every 512 samples (32 ms at 16 kHz),
`audio_analysis` works in 100 ms frames, so ~3.125 model outputs land in each
frame — most frames get 3, every eighth gets 4. Something has to pool them, and
_which_ something is a detection-quality decision, not a formatting one. All
three honest candidates are implemented (`shadow::PoolingRule`: `Max`, `Mean`,
`FractionOver`); `DEFAULT_POOLING` is `Max` **by argument, not by measurement**,
and the argument is written out at that constant: the pipeline below already
smooths twice, and `Max` is the only rule that does not throw evidence away
before the smoother can see it.

Note the unit change in `FractionOver`: it returns a **share**, which is then
read against a probability threshold. Coherent (`share ≥ 0.5` = "most of this
frame was speech") but a different question from the other two rules, and the
share is what becomes the frame's confidence and every confidence downstream.

Hops are assigned to the frame their **first sample** falls in, computed in
integer samples. Float comparison of `start_sec` against 0.1 s bounds puts an
exact quotient one ULP either side of the truth and gains or loses a hop —
another wrong answer that looks like a right one. Guarded by
`every_hop_lands_in_exactly_one_frame_and_none_is_lost`.

## The corpus: what the ground truth actually is

There is no benchmark and no fixture. The truth is **the corrections the owner
made**, in `<stem>.feedback.json` beside each recording
(`sundayrec_core::feedback`). That is the right truth for this app and it has one
sharp edge that anybody reading the harness has to know about.

The two correction kinds do not mean the same thing, and they do not correct the
same pick:

| Record             | Says                                         | Stored as      | Corrects                              |
| ------------------ | -------------------------------------------- | -------------- | ------------------------------------- |
| `sermon_picks`     | "you picked the wrong block, this one is it" | **absolute**   | `Detection::offered` (the editor's)   |
| `trim_adjustments` | "right block, wrong edges"                   | **two deltas** | `Detection::sermon` (the strict pick) |

A trim adjustment stores only the deltas — **the proposal it was measured against
is not stored with it.** Review's `suggested_trim` was built from the strict
pick in `prep::build_episode_prep` (gone with the review queue in R1
«Frivilligen først»; `Detection::sermon` is the same pick), so recovering what
the human actually meant means
re-deriving that proposal by running the heuristic again. That is valid, because
the detector is deterministic and the recording has not changed — but only as
long as the detector's boundary rule has not changed since the delta was
recorded, and nothing can check that.

So the harness grades provenance rather than flattening it
(`ab_eval::TruthStrength`):

- **`Direct`** — the record states the answer outright. Nothing reconstructed.
- **`Reconstructed`** — a delta, against a proposal re-derived by **this** build.
- **`ReconstructedAcrossVersions`** — a delta recorded by a _different_ build than
  the one re-deriving the proposal. Still counted, because dropping it would
  usually empty the corpus, but the report says out loud how many rows are of
  this kind, at the top level rather than buried in the rows: a corpus that is
  mostly this is a corpus whose boundary numbers rest on an unchecked assumption.

Two more consequences worth stating:

- A scorer is judged against **the pick the correction is a correction of**
  (`ScorerSelection::for_truth`). The two picks are usually the same block, so
  getting this wrong would be wrong only sometimes — the hardest kind of wrong to
  notice.
- **A recording with no correction is not a recording we got right.** The owner
  may simply never have opened it. Those are counted separately
  (`UncorrectedSummary`, carrying the literal code
  `not_evidence_of_correctness_no_human_looked` in the JSON) and never folded
  into an accuracy number.

## Honest limits

**Nothing synthetic in this repo separates the two detectors.** That is a
measured result, not an omission:

- On the synthetic voice, the model and the heuristic **agree** — same speech
  mass, same offer (`on_a_clean_utterance_the_two_detectors_agree`). A harness
  validated only against that would be validated against a tie.
- To manufacture a disagreement at all, the A/B work needed a **1 kHz pure tone**
  (`heuristic_speech_tone`, `src-tauri/src/vad/shadow.rs`). At 16 kHz it lands
  inside every one of the heuristic's speech windows — 2000 zero-crossings/sec
  inside 400..6000, a centroid of 1 kHz inside 300..3500, −9 dBFS inside
  −45..−5 — while its spectral flux is ~0, so `classify_frame` scores it speech 3,
  music 2, and calls it Tale. Silero hears no voice in it. That is the
  disagreement.
- **440 Hz does not work.** It wins the music ZCR test and comes out as Musikk,
  so both detectors reject it and there is nothing to compare. The fixture is not
  just "a tone"; if you change the frequency, re-read its doc comment first.

The generator behind `--synthetic` is likewise a plumbing check and says so in
every line of its own output. Its first version failed instructively: bare
impulses through parallel resonators scored 0.001, so the "comparison" exercised
only the found-nothing path while looking like it worked. Four things are
load-bearing for the model to bite — a _shaped_ glottal pulse, resonators in
**cascade** not parallel, formants that **glide**, and broadband noise at each
syllable onset.

**Real services are required.** Nothing below can substitute for them.

### How many recordings before anything can be said

The comparison is paired — both scorers see the same recording — so the test is a
two-sided sign test over the recordings where they disagreed.

- **6 — `MIN_CORPUS_FOR_ANY_CONCLUSION`.** The smallest possible p-value on `n`
  discordant pairs is `2 · 2⁻ⁿ`, reached only when every pair falls the same way.
  At n = 5 that is 0.0625: **a clean sweep still would not be significant.** At
  n = 6 it is 0.031. Six is the first size at which the question can be answered
  at all, and the tool refuses to express a direction below it. This is
  arithmetic, not judgement — `five_unanimous_recordings_cannot_reach_significance_but_six_can`
  asserts it rather than asserting it in prose.
- **12 — `MIN_CORPUS_FOR_A_DIRECTION`.** At n = 12 a 10–2 split reaches p = 0.039,
  so a _realistic_ lopsided result becomes distinguishable from a coin flip
  rather than only a perfect one (9–3 does not: p > 0.05). Twelve is also about a
  quarter of a year of Sundays, which is the least that can span the shapes a
  church year produces — an ordinary service, a high day with far more music, a
  short devotion, a concert. A detector tuned on one shape is not tuned.

Both are floors for _stating a conclusion_, not for running. The harness runs on
an empty corpus and reports what it has, loudly.

## The gate that has not been passed

> The VAD does not get to decide anything until the harness shows it at least
> matches the heuristic on the human-correction corpus.

`ab_eval::Verdict::VadAtLeastAsGood` is the only value that opens it, and
`decide` requires **all four** of:

1. **A sufficient corpus** (12 corrected recordings). Without it there is no
   result to have.
2. **Agreement no worse** — the VAD picks the human's block at least as often.
3. **Abstention no worse** — it does not buy accuracy by declining to answer. A
   scorer that finds nothing on the hard recordings has a beautiful error
   distribution over the easy ones.
4. **The error distribution no worse at every reported point, maximum included.**
   A detector whose median improves by two seconds while its worst case goes from
   30 s to four minutes has got worse at the only thing the operator will
   remember.

Passing three and failing the fourth is a real way to be worse, which is why each
is a separate condition with its own test. The asymmetry is deliberate: wrongly
holding the VAD back costs another season of corrections; wrongly letting it
through puts a detector nobody has evidence for in front of the only person who
will notice.

**And a second gate, which is not in the code and cannot be:** the owner has to
have listened to real church audio with the model's verdict in hand. Organ and
congregational singing against spoken liturgy is precisely Silero's hard case —
sustained, harmonic, human, and not speech. A corpus number that says "no worse"
over twelve services is necessary and not sufficient; somebody has to hear where
it puts the boundary on a hymn.

Today the expected verdict is `NotEnoughData`, and that is the correct answer,
not a failure.

## Cost

**~1.2 s per minute of audio**, single-threaded, release build — about **two
minutes for a 90-minute service**, on top of the analysis pass that just
finished. That is why shadow mode runs detached in `spawn_blocking` after
`editor_segments` has already answered: inline it would double a wait the
operator is watching, and no renderer listens to its progress event on purpose (a
second progress bar for work nobody is waiting on turns a background measurement
into something that looks like part of the job).

The engine is `tract`, chosen because it is **pure Rust** — no `-sys` crate, no
C/C++ toolchain, no system library — for an app that has to build unattended on
three platforms. The price is speed: tract runs roughly **6× slower than ONNX
Runtime** on this class of model. (That ratio is the general published figure, not
something measured in this repo; the 1.2 s/min above _is_ measured here.) At two
minutes per service, for an offline background pass, the trade is worth it. If
the VAD ever moves onto a path someone waits for, it stops being worth it and the
`VadBackend` seam is where you would change engines — that is what the seam is
for, and it is a one-file change in `src-tauri`.

The A/B harness sidesteps the cost differently: it scores each recording **once**
and pools those hops for every row of the sweep, because the hops do not depend
on the pooling settings. Three rules over a corpus therefore cost one inference
pass, not three.

## Where the code is

| File                                   | What                                                                       |
| -------------------------------------- | -------------------------------------------------------------------------- |
| `crates/sundayrec-core/src/vad.rs`     | Pure: framing, stream geometry, `VadBackend`, model provenance constants   |
| `src-tauri/src/vad/mod.rs`             | The only file naming an ONNX runtime. tract, the embedded model, the traps |
| `crates/sundayrec-core/src/shadow.rs`  | Pure: `PoolingRule`, `VadScorer`, the composition rule, `ShadowComparison` |
| `src-tauri/src/vad/shadow.rs`          | Shadow mode's I/O: decode, score, compare, write, log                      |
| `crates/sundayrec-core/src/ab_eval.rs` | Pure: the corpus judgement, the sweep, the verdict                         |
| `src-tauri/examples/vad_ab_eval.rs`    | The harness's I/O shell — the only caller of `ab_eval`                     |

Run the harness:

```text
cargo run --release --no-default-features --features vad --example vad_ab_eval -- \
    --dir ~/Documents/SundayRec
```

Exit 0 means the harness ran, whatever the verdict; "not enough data" is a
successful run. Exit 1 is an operational failure (no ffmpeg, unreadable
directory, a file that would not decode, a model that would not load) and no
report is written.
