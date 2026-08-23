# How SundayRec learns — signal → local record → aggregate → ritual → release

The operating manual for the one loop in this app that gets better because a
person disagreed with it. It is written so that somebody in six months can run
the whole thing without asking anyone: what a correction is, where it is kept,
what leaves the machine, how to read the aggregate, and what has to be true
before a detector constant is allowed to move.

Read it beside `docs/VAD.md`, which covers the other half — the neural detector
that is being measured against the same corpus and is not allowed to decide
anything yet.

## Status: the loop is built and has never run

Checked against the live admin API and the release tags on **2026-08-08**:

| Question                                          | Answer                                                                       |
| ------------------------------------------------- | ---------------------------------------------------------------------------- |
| Has any correction reached the aggregate?         | **No.** Zero events, zero installs, zero correction rows, zero outcomes.     |
| Does the STABLE ring's build capture corrections? | **No.** Stable serves `v0.10.0`, which has no `telemetry/` module at all.    |
| Which build does?                                 | `v0.11.0-beta.1` and later — `git tag --contains 96906c3` names exactly two. |
| Is anyone on that ring?                           | Not as of this writing.                                                      |

So every number this document describes is currently zero, and the reason is
structural rather than behavioural: the code that captures corrections is in the
beta ring only, and the beta ring is empty. That is worth stating precisely,
because "nobody has corrected anything" and "no build that could report a
correction is installed anywhere" look identical in the aggregate and mean
completely different things.

`scripts/tuning-report.mjs` prints both facts side by side for that reason.

Two branches are in flight against this same etappe and are referenced here by
name rather than described, because they are not merged:

- **`learn/e10-tuning-table`** — collects the detector's constants into one
  documented table in `crates/sundayrec-core/src/tuning.rs`, with golden tests.
  When it lands, "the constant" in the ritual below means a row in that table.
- **`learn/e10-local-adaptivity`** — bounded per-install nudging, i.e. a second
  learning path that never leaves the machine and therefore never appears in the
  aggregate this document is about. The two must not be confused: this loop
  changes what everyone's next release does; that one changes what one install
  does today.

---

## 1. Signal — what a person does that counts

Exactly three seams write a correction, and they are the three moments where the
app guesses and a person quietly fixes it:

| Seam                   | Command                              | What the human said                     |
| ---------------------- | ------------------------------------ | --------------------------------------- |
| The sermon dropdown    | `editor_record_sermon_pick`          | "you picked the wrong block, this one"  |
| Review's publish       | `learning::record_trim_deltas`       | "right block, wrong edges"              |
| The AI companion panel | `editor_record_companion_suggestion` | "this suggestion was / was not any use" |

All three go through one read-modify-write in `src-tauri/src/editor/mod.rs`,
serialised by `FEEDBACK_LOCK`, and all three call `observe_feedback_change` with
the file's projections **before and after** — never the event. That is not a
detail: `record_sermon_pick` REPLACES the previous answer to the same baseline,
so somebody auditioning block 2, then 3, then settling on 4 has made **one**
decision, and only the difference between two file states says so.

Two cases that deliberately do not count:

- **A boundary that moved less than `UNCHANGED_TOLERANCE_SEC` (0.5 s)** is the
  review round trip's own resolution, not a person disagreeing
  (`crates/sundayrec-core/src/trim_feedback.rs:82`). Counting those would bury
  the real corrections under a much larger number of artefacts all pointing the
  same way, which looks exactly like a signal.
- **Dragging a boundary back onto the proposal** is not "nothing happened" — it
  WITHDRAWS an adjustment recorded earlier. `record_trim_deltas`'s doc comment
  spells out why the unchanged case has to reach the function rather than being
  dropped by the caller: a record that still claims a correction the person has
  since taken back is not weak evidence, it is false evidence.

## 2. Local record — `<stem>.feedback.json`, beside the recording

`RecordingFeedback` (`crates/sundayrec-core/src/feedback.rs`) is the whole file.
Four collections, each with a bound and an explicit append-or-replace rule:

| Collection             | Bound | Rule                                   | Whose it is               |
| ---------------------- | ----- | -------------------------------------- | ------------------------- |
| `sermonPicks`          | 20    | replace per detector baseline          | the human's               |
| `trimAdjustments`      | 20    | replace per app version                | the human's               |
| `companionSuggestions` | 60    | **append** (3 per companion build)     | the human's               |
| `shadowObservations`   | 20    | replace per version + `ShadowSettings` | the app's, not a person's |

Both halves of each rule are load-bearing and they fail in opposite directions:
replacing where you should append loses the record of a genuinely separate
decision; appending where you should replace counts one person's one opinion as
many.

The record is precise because it is **local**. It sits next to the audio it
describes and is meaningless without it — offsets within a recording, never a
clock time. That is what makes it safe to keep in full, and it is also the
reason nothing of that precision may leave.

`FEEDBACK_SCHEMA` is 1 and adding a collection must NOT bump it: the reader
accepts one number and refuses any other, so a bump would make every file
already on disk unreadable, which here means stranding a human's work.

### Why a feedback record has no timestamp

`SermonPickCorrection`'s doc comment states the rule the two later records
inherit: no audio, no transcript, no recording name, no path, and **no
wall-clock time** — because a time of day next to a duration fingerprints one
service at one church. There is no field any of those could occupy. It is
structure, not filtering; nobody has to remember to strip anything.

That absence has a consequence which shapes the whole next stage, and it is the
non-obvious part of this design: **a record with no timestamp cannot be drained
against a watermark.** Every other telemetry source is drained against "the
newest record already reported", which is what makes a drain idempotent and its
schedule irrelevant. Corrections have nothing to put in one, so they cannot be
gathered by sweeping the recordings folder at drain time — that sweep would
re-report every correction on disk, every night, for as long as the recording
exists. The reasoning is written out in full in
`src-tauri/src/telemetry/corrections.rs`'s module header, together with the
price of the alternative it chose (a correction withdrawn across a drain
boundary leaves the fleet one over, saturating at zero — bounded, where the
sweep's error is unbounded and grows).

## 3. What leaves the machine, and what does not

| Local record           | On the wire                       |
| ---------------------- | --------------------------------- |
| `sermonPicks`          | signal + direction + band + count |
| `trimAdjustments`      | signal + direction + band + count |
| `companionSuggestions` | kind + outcome + count            |
| `shadowObservations`   | **nothing. Ever.**                |

A correction becomes a **signal** (which guess), a **direction** (which way),
and a **coarse band** (roughly how far) — and then a count of how many
corrections had that shape. Four signals × two directions × five bands = forty
possible facts and a number against each. That is the entire vocabulary; there
is no field on `CorrectionKey` or `CorrectionReport` a duration, a timestamp, a
name or a path could occupy.

The companion projection is the same discipline over a different collection:
kind (`title | description | chapters`) × outcome (`accepted |
accepted_edited | rejected | left_alone`), counted. `editedAfterAccept` does not
travel as its own field — it is folded into `accepted_edited` by
`CompanionOutcome::from_record`, so there is one value per fate rather than a
flag whose meaning depends on another field. The suggested title, the rewrite
and the transcript it came from have no field to occupy, on either side.

The band ladder is `under_15s | 15_30s | 30_60s | 60_120s | over_120s`,
half-open, lower bound inclusive. It is not a tuning choice: the Norwegian
privacy text gives the user the example «prekenstarten ble flyttet 30–60
sekunder tidligere», so `30_60s` must exist exactly, magnitude and direction
must be separate fields because they are separate words there, and nothing may
be finer — the neighbours are that band's successive doublings and both ends are
open. `the_example_from_the_privacy_text_lands_in_the_band_it_names` asserts the
sentence users were shown is still true.

**Shadow observations never leave, deliberately, against the programme's own
plan.** The consent text covers crash reports, quality data and feature-usage
counters; a disagreement between two of the app's own detectors is a fourth
category, and sending it would be collecting something nobody agreed to however
anonymous the numbers look. `feedback.rs`'s `ShadowObservation` doc comment says
so, and neither `telemetry::corrections` nor `telemetry::companion` reads that
collection. The A/B harness wants them locally anyway, which is where they are.

### Consent gates all of it

`CONSENT_VERSION` is **2**, and v2 exists precisely because corrections do: v1
promised counts, and a band is a magnitude, so reporting one is a new category
no matter how coarse. Two rules make the gate safe (`telemetry/consent.rs`):

1. **Absent means no.** A missing, unparseable or unrecognised record is "not
   granted". Nothing about telemetry is ever the default-on branch.
2. **A stale grant is not a grant.** Widening the scope stops sending and
   re-asks, rather than carrying an old yes forward.

The gate is checked at the cheapest place — `corrections::observe_files` returns
before projecting anything if consent is off — so a correction made by someone
who has not opted in costs two comparisons and is never accumulated at all.

### Where the code is stricter than `PRIVACY.md` — and the one place it is wider

`PRIVACY.md` is the promise. Read it as the specification. The code narrows it
in three places, and a gap in that direction is not a bug to be closed — closing
it would mean sending more than the text describes.

- **Movements under half a second are never reported.** The text promises coarse
  bands; the code additionally drops anything below `UNCHANGED_TOLERANCE_SEC`
  before a band is chosen (`band_delta`).
- **A pick with no auto-pick is never reported.** When the detector found no
  sermon at all and the human chose one, there is no proposal to have moved
  FROM, so there is no direction and no magnitude — and rather than invent one,
  the projection skips it. `trim_feedback::trim_deltas` refuses the identical
  case for the identical reason.
- **Shadow observations are outside the scope entirely**, as above. If central
  aggregation is ever wanted, that is a new consent decision and a
  `CONSENT_VERSION` bump, not a quiet addition to an existing payload.

Two places where the text and the storage are worth reading carefully together:

- **«En korrigering er den ene tingen vi samler inn som ikke er tidfestet i det
  hele tatt.»** True of the record: there is no time field on the wire and no
  `at` column in `migrations/0004_corrections.sql`, in either table. It is not
  quite true of the ROW: `event_corrections` is keyed by `event_id`, and
  `events.received_at` exists. What that timestamp dates is the **report**, not
  the correction — a correction made three weeks ago drains today — so the
  sentence is defensible, and the distinction is the one to state if anyone
  asks, rather than repeating the sentence.
- **Both correction families are named in the text** (resolved 2026-08-10, owner
  decision). `PRIVACY.md`'s correction section now describes the two acts
  separately — dragging the guessed start/end, and promoting a _different
  block_ (`sermon_pick_start` / `sermon_pick_end`) — and states that they are
  reported apart because they describe different failures. Nothing on the wire
  changed; the text caught up with it.

## 4. Aggregate — what the server keeps

`POST /v1/ingest` → `event_corrections` (raw) and, at the retention cutoff,
`agg_corrections` (forever). Two properties matter for the ritual:

- **Raw rows live 90 days; the fold is served separately.** `GET
/v1/admin/summary`'s correction query reads `event_corrections` only.
  Everything older survives as day totals in `agg_corrections`, served by `GET
/v1/admin/history` — a route the tool reads and folds in automatically, so a
  ritual run a season late still sees the whole corpus. The split is exact (the
  fold and the raw delete share one `db.batch()`), so summary + history is
  all-time with no overlap. On a Worker that predates the route the tool gets a
  404 and SAYS it is reading less than exists, with the `wrangler d1 execute`
  fallback named in the output.
- **`app_version` on an aggregate row is the REPORTING build**, not necessarily
  the build whose proposal was corrected (migration 0004 says so out loud). The
  local `TrimAdjustment` carries the right one; the wire does not, to avoid
  multiplying the key space by every version in the field. So "did the 0.11
  detector stop opening too late" is answerable, but not cleanly across an
  update boundary.

Read it with:

```text
node scripts/tuning-report.mjs          # the report
node scripts/tuning-report.mjs --json   # the raw summary, unrendered
```

The admin key comes from the macOS Keychain item `SundayRec telemetry admin key`
at run time — never an argument, never an env var, never printed. This is
`scripts/promote-release.mjs`'s `readAdminKey`, deliberately identical down to
the failure text. The tool is read-only by construction: one HTTP function, the
method hardcoded, and `scripts/tuning-report.test.mjs` asserts against the
file's own source that no other method and no other route appear in it.

Over an empty corpus it prints **no percentage at all** and says what is
missing, for the same reason `ab_eval`'s summary does: a "0 %" and a bar chart
of nothing both read as measurements, and there has been no measurement.
`an_empty_corpus_produces_a_report_that_says_so` asserts `'%'` never appears in
the harness's empty output; the script's test asserts the same of its own.

## 5. The ritual

Once there is something to read, this is the whole loop. It is five steps and a
release, and none of them is automatic.

1. **Read the aggregates.** `node scripts/tuning-report.mjs`. What you are
   looking for is a _direction skew within one signal_: not "people correct a
   lot", which says the editor is busy, but "68 out of 80 `sermon_start`
   corrections were `earlier`", which says the detector opens too late.
2. **Propose a change to a constant.** One constant, with the row that justifies
   it written into the commit message. Today's candidates live in
   `crates/sundayrec-core/src/detect.rs` (`MIN_SERMON_START_SEC`,
   `MIN_SERMON_DURATION_SEC`, `ATTENTION_CONFIDENCE_THRESHOLD`) and
   `audio_analysis.rs` (`SILENCE_DB`, `SMOOTH_HALF_WIN`, `MIN_SEGMENT_SEC`);
   `learn/e10-tuning-table` is collecting them into one table so that the
   candidate set is a list rather than a grep.
   **Note what is NOT there: there is no padding constant on the trim.**
   `prep::build_episode_prep` copied the sermon segment's own bounds into
   `suggested_trim` verbatim (R1 «Frivilligen først» removed the review queue
   and with it that prep; the rule — no padding — still holds for the editor's
   own proposal). So a systematic `sermon_start`/`earlier` skew is
   evidence about _segmentation_ — where a speech block is judged to begin — and
   not about a fudge factor somebody can nudge. That is the harder change, and
   pretending otherwise by adding a pad would be tuning the symptom.
3. **Run the golden tests.**
   `cargo test -p sundayrec-core --test detector_characterisation` replays 14
   fixtures through today's detector and demands the frozen answers in
   `fixtures/detector_golden.json`, except where `EXPECTED_DELTAS` demands they
   differ. A constant change that moves nothing is a constant that does not
   matter; a change that moves everything is one to be afraid of.
4. **Review the diff of which recordings changed answer.** This is the step that
   cannot be skipped and cannot be automated: the golden file's diff names the
   fixtures whose answer moved. Read them, not the pass/fail. If the VAD is in
   play, the same kind of change is graded against real corrections rather than
   fixtures by:

   ```text
   cargo run --release --no-default-features --features vad \
       --example vad_ab_eval -- --dir ~/Documents/SundayRec
   ```

5. **Ship it as a normal release.** Tag, build, publish, promote to `beta` with
   `scripts/promote-release.mjs`, watch, then `stable`. The change to detection
   rides in a version number like every other change.

### Explicitly NOT remote config

Serving detector constants from the Worker was considered and rejected, and the
Worker's routing table is the evidence that it stayed rejected: every route it
answers is listed in `sunday-telemetry/src/index.ts`'s header, and not one of
them serves configuration to a client. The only thing an install ever fetches is
an update manifest.

The reason is worth stating rather than assuming, because the feature is
genuinely attractive: it would let a tuning land the same afternoon, on
everyone, without a build.

**It would also mean the version number no longer tells you what the app did.**
Every support conversation this project has starts with "which version are you
on", and every bug report worth anything is a version plus a behaviour. With
remotely-tunable detection, two installs on `v0.12.0` can answer differently on
the same recording, a recording that was analysed last month cannot be
re-analysed to the same answer, and the golden tests above stop characterising
anything that ships. A detector nobody can reproduce is a detector nobody can
debug — and this one runs unattended, on a Sunday, in front of a volunteer.

The same argument does not forbid `learn/e10-local-adaptivity`, which is why
that branch exists: a bounded nudge computed **on the machine, from that
machine's own corrections**, is reproducible from things the machine can show
you. A number pushed from a server is not.

## 6. The evidence bar

`ab_eval` settled on **6** and **12**, and both are arithmetic rather than
judgement. The derivation lives at `MIN_CORPUS_FOR_ANY_CONCLUSION` and
`MIN_CORPUS_FOR_A_DIRECTION` in `crates/sundayrec-core/src/ab_eval.rs` and is
not restated here — `scripts/tuning-report.mjs` parses the two constants out of
that file rather than carrying copies, so there is one place they live and a
parse failure is a hard error rather than a default.

What does need saying is **why the same two floors govern this loop**, which
measures something different. `ab_eval` runs a two-sided sign test over
recordings where two scorers disagreed. A direction skew is a two-sided sign
test over corrections that fell one way or the other. Same test, same null,
therefore the same smallest-possible p-value on `n` observations — so the first
size at which any split can be told from chance, and the size at which a
realistic lopsided split rather than only a perfect one can, are the same two
numbers. `signTestP` in the script computes the exact p-value with BigInt (2⁻ⁿ
underflows a double past n ≈ 1074, and a p-value that silently becomes zero on a
large corpus is worse than none), and its tests pin the values `ab_eval`'s
doc comments name.

The floors are for _stating a conclusion_, not for running. Both tools run on an
empty corpus and report what they have, loudly.

## 7. What this loop cannot learn

Be concrete about this. Every item below is a real limit, not a caveat.

- **It sees corrections, not intentions.** A person dragging the start 40 s
  earlier may be fixing the detector, or trimming an announcement they did not
  want in the podcast, or cutting a false start. All three arrive as
  `sermon_start / earlier / 30_60s`. The band ladder cannot tell a disagreement
  from a preference.
- **A church that never corrects is not evidence the detector is right.** They
  may never have opened the editor. `ab_eval` refuses to fold those recordings
  into any accuracy number and carries the literal code
  `not_evidence_of_correctness_no_human_looked` in its JSON so the refusal
  survives being copied into a slide. The tool prints `editor.opened` and
  `review.published` next to the correction count for the same reason (the
  latter stopped being sent in R1 «Frivilligen først» — older clients' rows
  still carry it).
- **The aggregate cannot count churches.** A correction row carries no install
  id, by design (migration 0004) — that absence is what earns the aggregate
  indefinite retention. So forty corrections from one congregation and four each
  from ten are indistinguishable, and the sign test's independence assumption
  cannot be checked from this data. The tool says so on every run.
- **Nothing synthetic in this repo separates the two detectors.** That is a
  measured result, not an omission — `docs/VAD.md` records it: on the synthetic
  voice the model and the heuristic agree, and manufacturing a disagreement at
  all required a 1 kHz pure tone chosen so that it lands inside every one of the
  heuristic's speech windows while Silero hears no voice in it. A harness
  validated only against fixtures is validated against a tie. Real services are
  required.
- **It cannot see a correction that was never made because the operator gave up
  and edited elsewhere.** Somebody who exports the raw file and cuts it in a DAW
  produces no signal at all, and that is precisely the person the detector
  failed hardest.
- **It cannot attribute a correction to a build cleanly across an update.** See
  §4.
- **It says nothing about audio quality.** The corrections corpus grades where
  the sermon _is_, never how it _sounds_. Those are different failures with
  different telemetry (`event_quality`), and folding them together would produce
  a number that describes neither.

## Where the code is

| File                                                       | What                                                                 |
| ---------------------------------------------------------- | -------------------------------------------------------------------- |
| `crates/sundayrec-core/src/feedback.rs`                    | The local record: four collections, bounds, replace/append rules     |
| `crates/sundayrec-core/src/trim_feedback.rs`               | What a trim delta means, and which ones are real                     |
| `crates/sundayrec-core/src/telemetry/corrections.rs`       | The banded projection — the narrowing, and the band ladder           |
| `crates/sundayrec-core/src/telemetry/companion.rs`         | The suggestion-outcome projection (disjoint collection, same file)   |
| `crates/sundayrec-core/src/telemetry/consent.rs`           | The gate: three states, two rules, `CONSENT_VERSION`                 |
| `crates/sundayrec-core/src/ab_eval.rs`                     | The evidence bar and the corpus judgement                            |
| `src-tauri/src/editor/mod.rs`                              | The three seams, one lock, `observe_feedback_change`                 |
| `src-tauri/src/learning.rs`                                | Review's publish → trim deltas, infallible by signature              |
| `src-tauri/src/telemetry/corrections.rs`                   | The accumulator: consent mirror, subtract-on-drain, one settings row |
| `crates/sundayrec-core/tests/detector_characterisation.rs` | The golden record a tuning change is read against                    |
| `scripts/tuning-report.mjs`                                | Reads the aggregate; states the bar; says nothing over nothing       |
| `sunday-telemetry/migrations/0004_corrections.sql`         | The two tables, and why neither has a time column                    |
