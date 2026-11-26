# SundayRec — Smoke Test Runbook

A hands-on checklist for what the headless gate cannot prove: it needs a real
display, sound card, camera and (for the schedule) a machine that can sleep.
Written for the «rent bord» core: four pages — **Ta opp**, **Opptak**, **Plan**,
**Innstillinger**. Everything removed in «rent bord» (see [PLAN.md](PLAN.md)) has
no section here; older versions of this runbook are in git history.

> Legend: a `VERIFIED-BY:` line is a claim the automated suite already proves
> (`npm run smoke-verified` checks that the test still exists). What is left
> in each section is what a person must see on a rig. Open rig points are
> listed in [PLAN.md](PLAN.md) and walked in [RIG-DAY.md](RIG-DAY.md).

## 0. Prerequisites

- A Mac and a Windows PC, a real USB/ASIO interface (a mixer such as the Qu-5
  or X32 is the real test), a camera.
- `npm run ffmpeg` has fetched the sidecars; `npm run tauri dev` (or an
  installed build).
- macOS: microphone, camera and notification permission granted to SundayRec
  on first use (System Settings → Privacy & Security).

## 1. Pre-gate (headless, do this first)

```bash
npm run check          # format, lint, types, vitest, i18n, cargo fmt/clippy/test, bindings, reachability
npx playwright test    # renderer harness (start vite on :1420 first)
```

Expect both green before spending time on a rig.

## 2. First launch and first run

- [ ] Fresh install (no `onboardingDone`): the three-step first-run sequence opens; a settled install goes straight to **Ta opp**.
- [ ] Step 1 (sound card and channels): pick the real device, speak into the mic — the meter moves and «Neste» opens only once sound is heard. «Fortsett uten lyd» is the emergency exit.
- [ ] Step 2: «Velg mappe» opens the native folder dialog; a folder the app refuses (its own data folder) is refused with a sentence.
- [ ] Step 3: add a weekly slot in the week view — «Ta opp automatisk» and «Start automatisk med maskinen» switch on; «Senere» skips the step. Quit mid-sequence and relaunch: first run comes back.
- [ ] The consent card on **Ta opp** asks once; «Nei takk» is as good as «Ja».

- VERIFIED-BY: e2e/first-run.spec.ts::den første tiden slår på «Ta opp automatisk» og automatisk start
- VERIFIED-BY: e2e/first-run.spec.ts::bunnlinja forlater sekvensen, og chippen fører tilbake til steget man sto på
- VERIFIED-BY: e2e/onboarding.spec.ts::first run shows the wizard; a settled install does not
- VERIFIED-BY: e2e/onboarding.spec.ts::the consent step exists, and says what is and is not collected
- VERIFIED-BY: e2e/onboarding.spec.ts::«Ja, del anonymt» grants consent and finishes the wizard
- VERIFIED-BY: e2e/onboarding.spec.ts::«Nei takk» declines — and the app is otherwise identical
- VERIFIED-BY: e2e/onboarding.spec.ts::a backend that cannot record the answer does not trap the operator
- VERIFIED-BY: e2e/telemetry-preview.spec.ts::a payload carrying counters shows them, not «ingenting å sende»
- VERIFIED-BY: e2e/telemetry-preview.spec.ts::the one-time consent card asks, and a decline is recorded as a real answer

## 3. Ta opp — record 30 s

- [ ] Source and meter on the left, the big **Start** on a real sound card; the camera picture on the right when camera is on.
- [ ] Start → overlay → Stop (confirm) → a history row and a receipt with «Vis i Finder» / «Rediger».
- [ ] Long recording with «Del opp»: no audio lost at the file change (audio-only); a video recording has a short pause at the file change.
- [ ] Unplug the interface mid-recording: the dead-channel and device warnings say so in plain words.

- VERIFIED-BY: e2e/record.spec.ts::kamera på og valgt: bakenden bes om kameraet ved NAVN, og bildet fra økta står i rammen
- VERIFIED-BY: e2e/record.spec.ts::speilvendt opptak ⇒ speilvendt bilde: det som vises er det som tas opp
- VERIFIED-BY: e2e/record.spec.ts::nektet tilgang og «svarte ikke» er TO svar, ikke ett
- VERIFIED-BY: e2e/record.spec.ts::⚠️ bildet blinker IKKE ved Start og Stopp: én økt, aldri lukket, aldri «Starter kamera…»
- VERIFIED-BY: e2e/record.spec.ts::viser økta sitt bilde og måler seg på headeren
- VERIFIED-BY: e2e/recorder.spec.ts::manual start flips the app into the recording overlay
- VERIFIED-BY: e2e/recorder.spec.ts::a start the engine refuses keeps the modal open and says why
- VERIFIED-BY: e2e/record.spec.ts::kilden er valgt og til stede: Start kaller start_recording én gang — uten sti
- VERIFIED-BY: app/lib/api-shim-record.test.ts::sender aldri en sti — heller ikke når noen gir shimmen en
- VERIFIED-BY: src-tauri/src/commands/recorder.rs::a_renderer_supplied_path_never_reaches_the_engine
- VERIFIED-BY: src-tauri/src/commands/recorder.rs::golden_manual_start_matches_what_main_planned
- VERIFIED-BY: src-tauri/src/commands/recorder.rs::golden_manual_start_equals_the_old_plan_then_start_round_trip
- VERIFIED-BY: e2e/recorder.spec.ts::stop needs a one-second hold and then holds a finalizing overlay
- VERIFIED-BY: e2e/record.spec.ts::kvitteringens «Vis i Finder» handler på radens id fra hendelsen, ikke på en rad funnet via stien
- VERIFIED-BY: e2e/record.spec.ts::uten en rad er kvitteringens «Rediger» og «Vis i Finder» av, og sier hvorfor
- VERIFIED-BY: app/state/recording.test.ts::kvitteringen bærer radens id fra Rust, og en hendelse uten id gir ingen id
- VERIFIED-BY: src-tauri/src/scheduler/mod.rs::a_scheduled_start_with_video_on_hands_the_engine_the_camera_and_an_mp4
- VERIFIED-BY: crates/sundayrec-core/src/preflight.rs::the_missing_device_finding_is_generic_with_no_params
- VERIFIED-BY: crates/sundayrec-core/src/preflight.rs::the_card_and_the_notification_say_the_same_words
- VERIFIED-BY: app/pages/record/preflight-text.test.ts::enheten som mangler beholder akkurat den teksten den alltid har hatt

## 4. Opptak — list, editor and export

- [ ] The list shows the recording; a click opens the editor on the same page, the back arrow returns to the list. «Åpne fil…» and a dropped file open too.
- [ ] Waveform with two handles; cut, undo, play before/after. «Podkastklar lyd» on by default (−16 LUFS), off = as recorded.
- [ ] «Eksporter» opens the sheet **as a sheet** (focus lands in it and returns on close — check in WKWebView and WebView2). Intro/outro via the native file dialog.
- [ ] Trash tab: moved recordings arrive with their sidecar; «Tøm papirkurven» confirms with the count.

- VERIFIED-BY: e2e/library.spec.ts::«Vis i Finder» på en rad sender radens id til Rust, aldri stien
- VERIFIED-BY: src-tauri/src/commands/recordings_open.rs::a_recording_is_revealed_by_its_row_id_wherever_it_lies
- VERIFIED-BY: src-tauri/src/commands/recordings_open.rs::an_id_with_no_row_is_refused_and_a_path_is_no_id
- VERIFIED-BY: src-tauri/src/commands/recordings_open.rs::a_save_folder_in_the_apps_own_data_folder_is_refused
- VERIFIED-BY: app/pages/settings/folder-refusal.test.ts::en avvisning fra mappevinduet når fram til setningen
- VERIFIED-BY: src-tauri/src/commands/settings.rs::a_legacy_save_folder_the_vet_refuses_still_loads_saves_and_records
- VERIFIED-BY: src-tauri/src/commands/settings.rs::a_picked_save_folder_meets_the_real_vet
- VERIFIED-BY: src-tauri/src/commands/settings.rs::the_renderer_cannot_set_a_folder_the_vet_would_have_accepted
- VERIFIED-BY: e2e/settings-plan.spec.ts::«Velg mappe» ber Rust om mappevelgeren og viser det Rust lagret
- VERIFIED-BY: e2e/settings-plan.spec.ts::en mappe Rust avviser blir stående som den var, og setningen sier hvorfor
- VERIFIED-BY: e2e/export-sheet.spec.ts::«Vis i Finder» på kvitteringen sender lappen fra eksportens svar, aldri stien
- VERIFIED-BY: src-tauri/src/commands/recordings_open.rs::a_forged_foreign_or_misplaced_export_token_is_refused
- VERIFIED-BY: src-tauri/src/commands/recordings_open.rs::a_symlink_swapped_in_at_a_delivered_name_is_refused
- VERIFIED-BY: src-tauri/src/commands/trash.rs::a_recording_is_trashed_by_its_row_id_with_its_sidecar_and_nothing_else
- VERIFIED-BY: src-tauri/src/commands/trash.rs::an_id_the_history_does_not_know_refuses_the_whole_call
- VERIFIED-BY: e2e/retention.spec.ts::en flytting sier fra — toasten teller opptakene og «Vis papirkurven» går dit
- VERIFIED-BY: e2e/retention.spec.ts::en oppstart uten noe å flytte er stille
- VERIFIED-BY: src-tauri/src/commands/db.rs::prune_moves_into_the_trash_and_never_hard_deletes
- VERIFIED-BY: e2e/history.spec.ts::the search box filters live, and a miss says so in its own words
- VERIFIED-BY: src-tauri/src/editor/mod.rs::opening_a_recording_reads_it_once_for_every_consumer_or_skips
- VERIFIED-BY: src-tauri/src/editor/mod.rs::consumers_asking_at_once_share_one_read_or_skips
- VERIFIED-BY: src-tauri/src/editor/mod.rs::sidecars_answer_themselves_or_skips
- VERIFIED-BY: src-tauri/src/editor/mod.rs::the_round_agrees_with_ffmpeg_itself_or_skips
- VERIFIED-BY: src-tauri/src/editor/mod.rs::a_ninety_minute_recording_is_decoded_once_or_skips
- VERIFIED-BY: e2e/editor.spec.ts::a cut row shows its range and the ✕ really removes it
- VERIFIED-BY: e2e/editor.spec.ts::lyd-steget sier i tre ord hva «Tale» gjorde — og «rent» når ingenting
- VERIFIED-BY: e2e/editor.spec.ts::eksportsteget er ærlig om hvor filen havner
- VERIFIED-BY: src-tauri/src/editor/mod.rs::a_cut_only_video_export_copies_the_picture_and_keeps_every_cut_in_sync_or_skips
- VERIFIED-BY: src-tauri/src/editor/mod.rs::a_source_that_starts_late_is_copied_in_sync_too_or_skips
- VERIFIED-BY: src-tauri/src/editor/mod.rs::a_recording_with_far_apart_keyframes_is_re_encoded_exactly_or_skips
- VERIFIED-BY: src-tauri/src/editor/mod.rs::a_cut_shorter_than_the_keyframe_gap_is_not_swallowed_or_skips
- VERIFIED-BY: crates/sundayrec-core/src/video_copy.rs::a_start_snaps_back_to_the_keyframe_and_the_end_to_a_clean_split
- VERIFIED-BY: src-tauri/src/editor/mod.rs::a_power_cut_mid_export_is_reaped_from_a_picked_folder_on_the_next_launch_or_skips
- VERIFIED-BY: src-tauri/src/editor/mod.rs::startup_sweep_never_deletes_a_journalled_path_that_is_not_a_render_temp
- VERIFIED-BY: src-tauri/src/editor/mod.rs::startup_sweep_does_not_follow_a_journalled_symlink_or_touch_a_directory
- VERIFIED-BY: src-tauri/src/editor/export_journal.rs::a_temp_whose_folder_is_not_there_keeps_its_row_until_it_is_old
- VERIFIED-BY: src-tauri/src/commands/path_ratchet.rs::editor_export_names_its_folder_only_by_token
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_picked_folder_round_trips_as_a_token_and_shows_only_its_name
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_made_up_or_foreign_token_is_refused_with_its_own_code
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_token_to_a_folder_deleted_since_the_pick_is_refused
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_cancelled_pick_mints_nothing
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_path_in_the_old_folder_field_goes_nowhere
- VERIFIED-BY: src-tauri/src/commands/chosen_paths.rs::resolving_refuses_a_folder_swapped_for_a_symlink_elsewhere
- VERIFIED-BY: src-tauri/src/commands/chosen_paths.rs::the_store_is_bounded_and_evicts_the_oldest_first
- VERIFIED-BY: src-tauri/src/commands/chosen_paths.rs::a_minted_token_resolves_to_its_place_and_only_as_its_kind
- VERIFIED-BY: src-tauri/src/commands/chosen_paths.rs::the_same_place_again_reuses_its_token
- VERIFIED-BY: src-tauri/src/commands/chosen_paths.rs::the_vet_refuses_a_protected_folder_and_a_file_in_one
- VERIFIED-BY: src-tauri/src/commands/editor.rs::the_export_is_handed_the_resolved_places_and_nothing_the_webview_sent
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_same_folder_export_lands_exactly_where_it_did_before
- VERIFIED-BY: e2e/export-sheet.spec.ts::en valgt mappe er en lapp fra Rust: navnet vises, lappen sendes
- VERIFIED-BY: e2e/export-sheet.spec.ts::et avbrutt mappevalg lar «Samme mappe» stå, og ingen lapp sendes
- VERIFIED-BY: e2e/export-sheet.spec.ts::en mappe som er borte når eksporten starter, sier det med egne ord
- VERIFIED-BY: app/editor/export.test.ts::en mappe Rust avviser blir en setning, og målet bytter ikke i det stille
- VERIFIED-BY: src-tauri/src/commands/editor.rs::every_export_folder_refusal_has_a_sentence_in_the_renderer
- VERIFIED-BY: src-tauri/src/commands/settings.rs::a_picked_clip_is_stored_canonical_and_clearing_forgets_it
- VERIFIED-BY: src-tauri/src/commands/settings.rs::a_cancelled_clip_pick_changes_nothing
- VERIFIED-BY: src-tauri/src/commands/editor.rs::use_intro_reads_the_saved_clip_and_revalidates_it
- VERIFIED-BY: src-tauri/src/settings/mod.rs::settings_save_keeps_the_stored_intro_and_outro
- VERIFIED-BY: src-tauri/src/commands/media_filters.rs::the_jingle_picker_accepts_the_common_audio_formats
- VERIFIED-BY: src-tauri/src/editor/mod.rs::a_podcast_export_levels_intro_and_outro_like_the_sermon_or_skips
- VERIFIED-BY: src-tauri/src/editor/mod.rs::a_mastered_export_levels_the_sermon_not_the_jingles_or_skips
- VERIFIED-BY: e2e/export-sheet.spec.ts::intro og outro: velg, se navnet, slå av — og eksporten sender en bryter, ikke en sti
- VERIFIED-BY: e2e/export-sheet.spec.ts::intro og outro huskes til neste gang: etter en omlasting står de der
- VERIFIED-BY: e2e/export-sheet.spec.ts::en fil Rust avviser blir en setning, og forrige valg står
- VERIFIED-BY: e2e/export-sheet.spec.ts::en jingle som er borte når eksporten starter, sier det med egne ord
- VERIFIED-BY: e2e/export-sheet.spec.ts::med MP4-kortet valgt er jingle-flisene dempet og sier hvorfor, og ingen bryter sendes
- VERIFIED-BY: app/editor/jingles-core.test.ts::video får aldri jingler, selv med begge valgt
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_picked_recording_round_trips_as_a_token_and_shows_its_name
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_made_up_or_foreign_source_token_is_refused_with_its_own_code
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_token_to_a_recording_gone_since_the_open_is_refused
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_place_that_does_not_vet_grants_and_mints_nothing
- VERIFIED-BY: src-tauri/src/commands/editor.rs::the_webview_is_granted_exactly_the_canonical_file_it_opened
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_history_row_opens_by_its_id_and_the_webview_is_granted_its_file
- VERIFIED-BY: src-tauri/src/commands/editor.rs::an_id_with_no_row_is_unknown_and_a_path_is_no_id
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_history_row_whose_file_has_gone_says_so
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_dropped_file_opens_like_a_picked_one_and_carries_where_it_landed
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_dropped_folder_says_why_it_did_not_open
- VERIFIED-BY: src-tauri/src/commands/editor.rs::an_export_of_a_recording_that_is_not_open_is_refused_before_anything_else
- VERIFIED-BY: src-tauri/src/commands/editor.rs::an_old_shape_payload_names_no_source
- VERIFIED-BY: src-tauri/src/commands/editor.rs::every_source_refusal_has_a_sentence_in_the_renderer
- VERIFIED-BY: src-tauri/src/commands/media_filters.rs::every_language_names_the_media_filters
- VERIFIED-BY: src-tauri/src/db/store.rs::a_row_gives_its_file_by_id_and_a_made_up_id_gives_nothing
- VERIFIED-BY: src-tauri/src/commands/editor.rs::the_same_file_exports_to_the_same_place_through_a_token_as_by_its_path
- VERIFIED-BY: src-tauri/src/commands/editor.rs::use_intro_with_no_saved_clip_is_no_intro
- VERIFIED-BY: src-tauri/src/commands/settings.rs::a_pick_that_is_not_a_file_stores_nothing
- VERIFIED-BY: src-tauri/src/commands/path_ratchet.rs::the_requests_name_the_recording_only_by_token
- VERIFIED-BY: src-tauri/src/commands/path_ratchet.rs::editor_open_known_takes_only_a_history_row_id
- VERIFIED-BY: src-tauri/src/commands/path_ratchet.rs::every_token_is_minted_by_a_listed_function_behind_a_listed_door
- VERIFIED-BY: src-tauri/src/commands/path_ratchet.rs::a_row_lookup_whose_answer_is_ignored_does_not_anchor_a_mint
- VERIFIED-BY: src-tauri/src/commands/editor.rs::the_asset_grant_opens_the_one_file_and_nothing_beside_it
- VERIFIED-BY: src-tauri/src/commands/editor.rs::the_static_asset_scope_allows_no_folder_and_keeps_its_deny_list
- VERIFIED-BY: src-tauri/src/recorder/recovery.rs::a_manifest_under_a_name_the_recorder_does_not_write_gives_no_row
- VERIFIED-BY: src-tauri/src/recorder/recovery.rs::a_manifest_that_names_a_protected_file_gives_no_row_and_deletes_nothing
- VERIFIED-BY: src-tauri/src/commands/path_ratchet.rs::a_command_that_mints_a_path_from_the_webview_is_found
- VERIFIED-BY: src-tauri/src/commands/path_ratchet.rs::editor_export_hands_the_seam_the_resolved_places
- VERIFIED-BY: src-tauri/src/commands/path_ratchet.rs::an_export_that_ignores_the_resolved_places_is_found
- VERIFIED-BY: src-tauri/src/commands/path_ratchet.rs::a_guard_named_only_in_a_string_literal_does_not_count
- VERIFIED-BY: e2e/editor.spec.ts::«Rediger» på en biblioteksrad åpner opptaket
- VERIFIED-BY: e2e/editor.spec.ts::kvitteringens «Åpne i Rediger» åpner opptaket som nettopp ble tatt opp
- VERIFIED-BY: e2e/editor.spec.ts::«Etter» spilles av motoren, og Før/Etter er et bytte — ingenting gjengis
- VERIFIED-BY: e2e/library.spec.ts::«Fant ikke fila», med en vei til papirkurven — ikke den generiske korrupt-teksten
- VERIFIED-BY: app/editor/export-core.test.ts::opptakslappen har setninger: åpne på nytt, borte, avvist — og jingelen
- VERIFIED-BY: app/editor/loader-core.test.ts::kjenner source_missing: lappen peker på en fil som er borte (A2)
- VERIFIED-BY: e2e/editor.spec.ts::unsaved cuts from a previous session come back on reopen
- VERIFIED-BY: app/editor/cuts.test.ts::skrives med opptakets lapp, aldri stien
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_sidecar_is_kept_read_and_removed_beside_the_recording_its_token_stands_for
- VERIFIED-BY: src-tauri/src/commands/editor.rs::a_sidecar_command_with_a_forged_token_never_touches_the_disk

## 5. Plan — week view and wake

- [ ] Drag in an empty cell to add a slot (quarter-hour snapping), drag a block to move it, drag an edge to stretch; click opens the small editor. Keyboard: arrows, Shift + arrows, Enter, Delete.
- [ ] Touch: drag works on a touch screen / pen in the real Tauri window (not just a mouse).
- [ ] Scheduled start/stop on a real machine: a slot a few minutes ahead starts and stops by itself; the machine wakes from sleep for it (macOS admin prompt once; Windows wake timers).
- [ ] «Test vekking» is blocked during a recording, with a reason.

- VERIFIED-BY: src-tauri/src/scheduler/mod.rs::a_stop_with_nothing_recording_is_not_announced
- VERIFIED-BY: crates/sundayrec-core/src/wake.rs::a_wake_that_cannot_be_armed_is_told_to_the_operator
- VERIFIED-BY: src-tauri/src/scheduler/mod.rs::missed_check_looks_back_a_week_and_no_further
- VERIFIED-BY: src-tauri/src/wake/plan.rs::mac_elevated_plan_is_byte_exact_for_a_normal_schedule
- VERIFIED-BY: src-tauri/src/wake/plan.rs::mac_elevated_plan_keeps_a_hostile_owner_label_inside_the_literal
- VERIFIED-BY: src-tauri/src/wake/mod.rs::mac_partial_failure_escalates_to_one_admin_prompt_for_the_whole_set
- VERIFIED-BY: src-tauri/src/wake/mod.rs::mac_admin_prompt_dismissal_is_cancelled_not_permission
- VERIFIED-BY: src-tauri/src/wake/mod.rs::windows_clears_then_arms_one_waitable_timer_per_point
- VERIFIED-BY: src-tauri/src/wake/mod.rs::engine_records_the_empty_key_so_a_re_add_re_registers
- VERIFIED-BY: src-tauri/src/wake/mac_read.rs::live_iokit_read_is_callable_unprivileged
- VERIFIED-BY: e2e/plan-week.spec.ts::å dra i en tom flate lager en tid, rundet til kvarter
- VERIFIED-BY: e2e/plan-week.spec.ts::å dra en blokk til en annen dag og et annet klokkeslett flytter den
- VERIFIED-BY: e2e/plan-week.spec.ts::bunnkanten endrer slutten, toppkanten starten
- VERIFIED-BY: e2e/plan-week.spec.ts::trykk på den valgte blokken åpner redigeringen; Lagre skriver eksakte tider og dag
- VERIFIED-BY: e2e/plan-week.spec.ts::Slett fjerner tiden
- VERIFIED-BY: e2e/plan-week.spec.ts::piler flytter, Skift + piler strekker, Enter åpner, Delete fjerner
- VERIFIED-BY: e2e/plan-week.spec.ts::overlapp avvises med en setning, og tiden står der den var
- VERIFIED-BY: e2e/plan-week.spec.ts::en tid over midnatt tegnes i to kolonner og kan redigeres
- VERIFIED-BY: e2e/plan-week.spec.ts::«Neste opptak» er fremhevet, og savnede opptak denne uka er merket
- VERIFIED-BY: e2e/status-honesty.spec.ts::under et opptak rører ikke minuttpollen wake_verify i det hele tatt
- VERIFIED-BY: e2e/status-honesty.spec.ts::et scheduler-neste-event UTENFOR opptak henter en fersk wake_verify
- VERIFIED-BY: e2e/advanced.spec.ts::planlegger en ekte vekking, og «Avbryt» tar raden tilbake
- VERIFIED-BY: e2e/advanced.spec.ts::en feilet test siterer DEN SAMME setningen som «Aktiver vekking»
- VERIFIED-BY: e2e/advanced.spec.ts::«Test vekking» er sperret mens et opptak går, med en grunn
- VERIFIED-BY: e2e/advanced.spec.ts::feilhistorikken viser hendelsene, og «Tøm» kaller SLETTEKOMMANDOEN
- VERIFIED-BY: crates/sundayrec-core/src/settings.rs::slots_round_trip_through_json

## 6. Innstillinger

- [ ] Lyd og kanaler: device, channel pairs, the meter. A silent channel in the editor shows the dead-channel warning (check with a real mixer).
- [ ] Camera, storage folder and format, notifications (OS permission off says so), language (no/en), recording options, updates (channel stable/beta), privacy (telemetry preview), log («Kopier siste logg») and licenses.
- [ ] Windows only: the database moves from Roaming to Local AppData on first launch after the update; a crash manifest from before the move is recovered.

- VERIFIED-BY: e2e/settings-plan.spec.ts::opptaksformatet lagrer BÅDE format og bitrate, og seksjonen står der
- VERIFIED-BY: e2e/settings-plan.spec.ts::?goto=settings:sound lander på Lyd og kanaler
- VERIFIED-BY: e2e/settings-plan.spec.ts::«Endre» på kilden fører til Lyd og kanaler i Innstillinger
- VERIFIED-BY: e2e/settings-plan.spec.ts::opptaksstart river måleren ut av TA OPP
- VERIFIED-BY: src-tauri/src/notify/permission.rs::windows_is_on_unless_a_switch_says_off
- VERIFIED-BY: e2e/settings-plan.spec.ts::varsler slått av i OS-et sier det, og peker på bryteren
- VERIFIED-BY: crates/sundayrec-core/src/notify.rs::a_take_alert_reaches_the_os_only_when_nobody_is_looking
- VERIFIED-BY: src-tauri/src/notify/take.rs::a_new_take_lets_every_kind_be_said_again
- VERIFIED-BY: app/lib/api-shim-files.test.ts::en mappe som ikke finnes ennå, sies med vanlige ord — og huskes
- VERIFIED-BY: src-tauri/src/appdata.rs::en_flytting_tar_med_skrivinger_som_bare_la_i_wal
- VERIFIED-BY: src-tauri/src/appdata.rs::flyttingen_sletter_ikke_roaming_og_den_gamle_fila_har_alt
- VERIFIED-BY: src-tauri/src/appdata.rs::andre_oppstart_flytter_ikke_igjen
- VERIFIED-BY: src-tauri/src/appdata.rs::en_feil_i_flyttingen_gir_fallback_til_roaming_uten_tap
- VERIFIED-BY: src-tauri/src/appdata.rs::en_local_mappe_som_ikke_kan_lages_gir_fallback_uten_tap
- VERIFIED-BY: src-tauri/src/recorder/recovery.rs::et_manifest_fra_et_krasj_for_oppdateringen_gjenopprettes_etter_flyttingen
- VERIFIED-BY: src-tauri/src/recorder/recovery.rs::manifester_i_begge_mappene_gjenopprettes_hver_for_seg
- VERIFIED-BY: src-tauri/src/appdata.rs::en_roaming_mappe_hvis_forelder_mangler_gir_fallback_og_ingenting_i_local
- VERIFIED-BY: src-tauri/src/appdata.rs::en_metadata_feil_som_ikke_er_notfound_gir_fallback_og_ingenting_i_local
- VERIFIED-BY: src-tauri/src/appdata.rs::en_rename_som_feiler_et_par_ganger_prøves_på_nytt_og_flyttingen_lykkes
- VERIFIED-BY: src-tauri/src/appdata.rs::en_roaming_database_nyere_enn_local_gir_varsel_om_data_igjen_i_den_gamle_mappa
- VERIFIED-BY: src-tauri/src/appdata.rs::utenfor_windows_er_stien_uendret_og_ingenting_leses_eller_lages
- VERIFIED-BY: src-tauri/src/appdata.rs::mac_og_linux_har_samme_appdata_sti_som_for
- VERIFIED-BY: e2e/system-support.spec.ts::«Kopier siste logg» puts the tail on the clipboard and confirms
- VERIFIED-BY: e2e/system-support.spec.ts::an empty log is called out instead of copying nothing
- VERIFIED-BY: e2e/system-support.spec.ts::«Vis lisenser» opens the bundled notices in a frame and closes again
- VERIFIED-BY: e2e/auto-update.spec.ts::off at startup: zero update_check even while settings load slowly (the #11 race)
- VERIFIED-BY: e2e/auto-update.spec.ts::on at startup: one immediate check, and the hourly repeat is scheduled
- VERIFIED-BY: e2e/auto-update.spec.ts::toggling off while running stops the schedule and further checks
- VERIFIED-BY: e2e/auto-update.spec.ts::toggling back on re-arms: an immediate check and a fresh schedule
- VERIFIED-BY: e2e/update-channel.spec.ts::switching to beta reaches the store, not just the select
- VERIFIED-BY: e2e/update-channel.spec.ts::switching back to stable syncs too, and asks no question
- VERIFIED-BY: src-tauri/src/commands/recordings_open.rs::the_webview_holds_no_updater_permission
- VERIFIED-BY: src-tauri/src/commands/recordings_open.rs::nothing_the_webview_is_built_from_names_the_updater_plugin
- VERIFIED-BY: src-tauri/src/commands/recordings_open.rs::the_build_script_no_longer_writes_an_updater_capability
- VERIFIED-BY: e2e/auto-update.spec.ts::a feature_disabled check surfaces as the ordinary error text
- VERIFIED-BY: e2e/auto-update.spec.ts::an upToDate answer paints «Du er oppdatert» and retires stale buttons
- VERIFIED-BY: e2e/settings.spec.ts::the language setting round-trips into storage and survives a reload
- VERIFIED-BY: e2e/settings-seam.spec.ts::boot performs no settings_save at all
- VERIFIED-BY: e2e/settings-seam.spec.ts::one change sends only that field — untouched fields keep their stored values
- VERIFIED-BY: crates/sundayrec-core/src/settings.rs::legacy_blob_with_v015_dead_fields_imports_cleanly
- VERIFIED-BY: src-tauri/src/commands/path_ratchet.rs::every_command_that_parses_a_string_into_settings_is_on_the_closed_list
- VERIFIED-BY: crates/sundayrec-core/src/settings.rs::removed_church_publish_and_special_fields_are_ignored_and_not_rewritten
- VERIFIED-BY: crates/sundayrec-core/src/tray.rs::the_tray_carries_no_system_check_item

## 7. Safety rails (path and token ratchets, startup sweeps)

Nothing to do by hand: these claims are proved by the suite and listed so a rename cannot silently drop them.

- VERIFIED-BY: e2e/system-support.spec.ts::a missing notices file says so instead of showing an empty page

## What «passed» means

Every box above ticked on one Mac and one Windows PC, `npm run check` green on
the commit under test, and the result written into [PLAN.md](PLAN.md) (what was
proved, on which machine, which version).
