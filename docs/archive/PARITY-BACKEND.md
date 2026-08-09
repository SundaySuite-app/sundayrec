# Frontend-parity — the two backend-dependent features

> **STATUS: BOTH DONE (2026-06-03).**
>
> - #1 Channel-select L/R — commit `91846b4`.
> - #2 Auto-stop countdown + extend/cancel — commit `c6eeec1`.
>
> Both shipped behind a green `npm run check` (the full Rust test suite +
> clippy `-D warnings`; the frontend has no JS unit-test harness on this branch). The implementations follow the threads below, with two
> deliberate refinements: (1) the channel pick is one shared L/R pair (not a
> per-device `HashMap`); (2) auto-stop uses a `tokio::sync::watch` channel with
> an absolute epoch-ms deadline (not a re-baked duration), so extend/cancel move
> the REAL timer immediately and splits/reconnects re-pin the same stop time.
> `extend` ADDS to the live deadline (so "+30 min" never shortens). Still
> HARDWARE/TIMING-UNVERIFIED until a rig smoke-test.

Scoped by parallel design agents + validated against the code. These are the
last two parity items; both need backend changes (hence not done in the
frontend-only pass). The smaller polish that came out of the same agent batch
(Home VU peak-hold + clip) is already landed (commit 9a06b97).

## 1. Channel-select L/R (multi-channel mixers) — SAFER, do first

Pick which device input channels to record (e.g. X32 channels 17 & 18). Today
only `ChannelMode` (stereo/monoL/monoR/monoMix) exists; no per-channel pick.

**Thread (Settings → recorder → ffmpeg pan):**

1. `crates/sundayrec-core/src/settings.rs` — add `input_channel_l: Option<i32>` +
   `input_channel_r: Option<i32>` (`#[serde(default)]`), 0-based. Follow the
   `video_flip` pattern exactly: struct field + `Default` impl + clamp in
   `validate()` (0..=31) + the static-default object in
   `src/design/screens/settings.helpers.ts` + regen bindings. (Simpler than the
   agent's per-device `HashMap` — one mixer is the common case; revisit if needed.)
2. `crates/sundayrec-core/src/capture.rs`:
   - new pure fn `custom_channel_map_filter(mode, l, r) -> Option<String>`:
     for `Stereo` with `(l,r) != (0,1)` → `Some(format!("pan=stereo|c0=c{l}|c1=c{r}"))`,
     else `None` (mono modes keep `channel_map_filter`'s existing pan — device
     routing for mono is HARDWARE-UNVERIFIED, defer).
   - add `input_channel_l/r: Option<i32>` to `CaptureOpts` (struct ~L297 + Default ~L337).
   - at **L393** (`let pan = channel_map_filter(...)`) use the custom filter when
     channels are set, falling back to the mode default.
   - unit test: `pan=stereo|c0=c16|c1=c17` for channels (16,17); clamps; mono→None.
3. Recorder thread: `src-tauri/src/recorder/engine.rs` `RecordingOpts` (~L153) gains
   the two fields; `CaptureOpts {…}` (~L264) passes them. Every `RecordingOpts`
   builder must set them: `src-tauri/src/scheduler/mod.rs:429` (`channel_mode:
settings.channels` → also `input_channel_l: settings.input_channel_l, …`) + the
   manual-start command + the test constructors (engine.rs ~L1758/2149).
4. Frontend: `src/design/screens/SettingsScreen.tsx` `TabLydkilde` (~L314) — a
   `ChannelSelectCard` shown only when the selected device has `channels > 2`
   (use `useInputDevices()` for the count): two `<select className="sr-select">`
   for L/R (options 1..channels, 0-based values), via the tab's `update()`.
5. i18n: `settingsScreen.audio.{inputChannelsTitle,inputChannelsDesc,channelL,channelR,channel,inputChannelsHint}` × 7 + a SettingsScreen render test.

## 2. Auto-stop countdown + extend/cancel — RISKY, do carefully

⚠️ The recorder has **no scheduled-stop concept** today — only `manual_max_minutes`
(a duration baked into a `tokio::Sleep` at session start; no live extend/cancel).
This touches the live recording engine — the highest-risk change in the app.

**Plan:**

1. `src-tauri/src/recorder/engine.rs`:
   - `RecorderStatePayload` (~L237) gains `scheduled_stop_ms: Option<u64>`
     (`#[ts(type="number | null")]`). Regen bindings.
   - `RecorderEngine` gains `scheduled_stop_ms: Arc<Mutex<Option<u64>>>`.
   - on session start, if `manual_max_minutes > 0` (and/or a scheduled slot's
     absolute stop — VERIFY the scheduler: does a slot carry a stop time or just a
     duration?), set `now_ms() + minutes*60_000` and emit it.
   - **the hard part:** the `tokio::select!` auto-stop branch must read the
     _mutable_ `scheduled_stop_ms` each iteration (a dynamic `sleep_opt(remaining)`),
     so extend/cancel actually move/clear the real timer — not just the UI. Verify
     split-segment sessions re-pin the same absolute stop, not a fresh duration.
2. `src-tauri/src/commands/recorder.rs`: `recording_extend_autostop(minutes)`
   (sets `now_ms()+minutes*60_000`, re-emits state) + `recording_cancel_autostop()`
   (sets `None`, re-emits). Register both in `src-tauri/src/lib.rs` invoke_handler.
3. Frontend `src/design/screens/RecordingScreen.tsx`: `useRecordingSession` tracks
   `scheduled_stop_ms` from the state event + a 1 s ticking countdown; render an
   `AutoStopCard` ("Auto-stopp om HH:MM:SS" + "+30 min" → extend + "Avbryt
   auto-stopp" → cancel) when set. i18n `recordingScreen.{autoStopLabel,extend30,
cancelAutoStop}` × 7. Unit-test the extend/cancel math (pure) + the card render.

Both: keep `npm run check` green; HARDWARE/timing-UNVERIFIED until a rig smoke-test.
