//! What a settings PROFILE carries — the JSON file «Innstillingsprofil»
//! exports and imports (the dialogs and the file I/O are in
//! `commands::settings`).
//!
//! A profile carries **the church's way of recording**: the language, the
//! format and its quality, the file names, the schedule, the silence and
//! length rules, the church's name, «Legg ut». It does NOT carry **this
//! machine**: its sound card, its camera, where it writes, how it starts and
//! updates. Those are [`MACHINE_LOCAL`]: left out of every export, and ignored
//! in every import, so the machine importing keeps its own.
//!
//! ## Over what is stored, not over the defaults
//!
//! [`super::import`] (the localStorage hand-over) rebuilds the whole object
//! from the blob and the DEFAULTS — right before anything is stored, wrong on
//! a machine that has been recording for a year. Through it, any file that was
//! not a settings object (a recording, a PDF, `{}`) became the full defaults:
//! the folder, the language and the schedule wiped, and the toast said
//! «Innstillingene ble importert». So a profile is laid over the STORED
//! settings ([`overlay_profile`]), and a file that names none of them is
//! refused with `profile_not_settings`, nothing written.
//!
//! ## Why the machine-local line is drawn where it is (review of #309)
//!
//! The case the feature has to survive is not the happy one (the church PC
//! exports, a second machine imports). It is a full profile exported from a
//! laptop that was never set up, imported on the church PC on Saturday
//! evening. Every key is present in such a file, every value a default — and
//! before this module, every one of them was taken:
//!
//! - `launchAtLogin` true → false: the shell's boot sync REMOVED the OS login
//!   item, and after Saturday night's update reboot SundayRec did not start;
//! - `deviceId`/`deviceName` → none: the service was recorded from the
//!   system's default input, not the mixer;
//! - `deviceChannels`/`inputChannelL`/`R` → none: an X32's 16/17 routing gone;
//! - `videoEnabled` → off;
//! - `autoDeleteDays` 0 → 14: retention moved recordings to the papirkurv
//!   within twelve hours, past the confirmation Avansert asks for — while the
//!   import's own question promised that recordings were not touched.
//!
//! ## What an import may never take away
//!
//! Beyond the machine-local fields, three carried settings are one-way
//! ([`never_take_away`]): an EMPTY schedule or special-recordings list keeps
//! this machine's; an import may arm the weekly plan but never disarm it; and
//! automatic deletion may be switched off or made to wait longer, never
//! switched on or shortened. Each is a setting where a file's default
//! silently costs a recording.
//!
//! ## Why machine-local fields are left out of the EXPORT too
//!
//! The import ignores them, so a file that carried them would only mislead:
//! whoever reads it believes the sound card travels. And they are the
//! settings that name this machine — a user name inside a save folder or an
//! intro clip's path, a sound card's id — in a file that is made to be handed
//! to someone else. (A profile written before this change still carries them;
//! the import ignores them all the same.)

use serde::Deserialize;
use serde_json::Value;
use sqlx::SqlitePool;
use sundayrec_core::publish::PublishTarget;
use sundayrec_core::settings::Settings;

use super::{load, save};
use crate::error::{AppError, AppResult};

/// Settings that describe THIS machine rather than the church's way of
/// recording, by their serialised (camelCase) name. Never exported, never
/// imported: an import keeps this machine's value for every one of them.
/// `every_machine_local_key_is_a_settings_field` holds the list to the model,
/// so a renamed field cannot silently fall out of it.
pub const MACHINE_LOCAL: &[&str] = &[
    // ── The sound card and how it is wired and clocked. A laptop's built-in
    //    microphone is not the church's mixer, and the routing is keyed by the
    //    device: carrying any part of the group would mix one machine's device
    //    with another's channels (or name with id). All seven or none.
    "deviceId",
    "deviceName",
    "deviceChannels",
    "inputChannelL",
    "inputChannelR",
    // Mono-left vs stereo is how THIS rig's mic is cabled into its inputs; a
    // laptop's «stereo» puts the pulpit mic on one side only.
    "channels",
    // A forced rate is one THIS interface supports; another card may refuse it.
    "sampleRateMode",
    // ── Per-rig escape hatches for the capture engine ("flip on only if the
    //    native engine misbehaves on a specific rig" — their own docs).
    "classicDirectshow",
    "classicFfmpegAudio",
    "classicFfmpegPreroll",
    // ── The camera: which one, whether it records, and how it is mounted
    //    (`videoFlip` is "a per-machine preference" in its own doc).
    "videoEnabled",
    "videoDeviceName",
    "videoDeviceIndex",
    "videoFlip",
    // ── Where this machine writes. A folder in a profile is a path on the
    //    machine that exported it: another user's home, a drive letter or a
    //    `/Volumes/…` stick this machine does not have — accepted by the
    //    folder vet (a folder that does not exist yet can only be judged by
    //    its name) and found unwritable on Sunday.
    "saveFolder",
    // ── Files on the exporting machine: intro/outro clips the editor would
    //    fail to find here.
    "editorIntroPath",
    "editorOutroPath",
    // ── How this machine starts and wakes. The shell syncs `launchAtLogin`
    //    into the OS login item at boot; `wakeFromSleep` into the OS wake
    //    schedule. Either one switched off by a file is a Sunday with no app.
    "launchAtLogin",
    "wakeFromSleep",
    // ── This installation: whether it has been through first-run (a new
    //    machine still has to pick its own sound card there), and how it
    //    updates itself (the beta ring "is opted into per machine").
    "onboardingDone",
    "autoUpdate",
    "updateChannel",
];

/// Settings that describe the church's WAY of recording — they travel in a
/// profile, both ways. Together with [`MACHINE_LOCAL`] and [`ONE_WAY`] this is
/// every `Settings` key exactly once; `every_settings_field_is_classified`
/// fails the day somebody adds a field and does not say which it is (a new
/// path, device, identity or consent switch must NOT land here by default).
pub const SHARED: &[&str] = &[
    "language",
    "keepSeparateAudio",
    "format",
    "bitrate",
    "filenamePattern",
    "stopOnSilence",
    "silenceThreshold",
    "silenceTimeoutMinutes",
    "splitMinutes",
    "manualMaxMinutes",
    "preRollSeconds",
    "prerollEnabled",
    "reminderMinutes",
    "protectRecording",
    "churchName",
    "responsiblePerson",
    "publishTarget",
    "publishCustomUrl",
    "publishDescriptionTemplate",
    "notifyStart",
    "notifyStop",
    "askOpenEditor",
];

/// Carried, but an import may only move them one way — see
/// [`never_take_away`]. (`specialRecordings` also has a machine-local part:
/// each entry's `deviceId`, see [`without_special_devices`].)
pub const ONE_WAY: &[&str] = &[
    "autoDeleteDays",
    "autoRecordEnabled",
    "slots",
    "specialRecordings",
];

/// A strict reading of one field's value: `true` = it is what the field holds.
type StrictCheck = fn(&Value) -> bool;

/// Carried fields whose `Settings` deserializer is LENIENT — it turns a value
/// it cannot read into the default instead of failing. Through the overlay
/// that would be garbage silently resetting the field, and counting as "a
/// setting the file names". So each is checked strictly first; a value that
/// does not pass is skipped like any other unreadable field.
/// `every_lenient_field_is_machine_local_or_checked_strictly` holds this list
/// to the model's `deserialize_with` attributes.
const STRICT: &[(&str, StrictCheck)] =
    &[("publishTarget", |v| PublishTarget::deserialize(v).is_ok())];

/// The current settings as a profile: pretty JSON, without [`MACHINE_LOCAL`].
pub async fn export_profile(pool: &SqlitePool) -> AppResult<String> {
    profile_json(&load(pool).await?)
}

/// `settings` as a profile — [`export_profile`] without the database.
fn profile_json(settings: &Settings) -> AppResult<String> {
    let mut value = serde_json::to_value(settings)?;
    if let Value::Object(fields) = &mut value {
        for key in MACHINE_LOCAL {
            fields.remove(*key);
        }
        // The nested half of the machine-local line: a special recording's own
        // sound card is this machine's too.
        if let Some(Value::Array(specials)) = fields.get_mut("specialRecordings") {
            for special in specials {
                if let Value::Object(special) = special {
                    special.remove("deviceId");
                }
            }
        }
    }
    Ok(serde_json::to_string_pretty(&value)?)
}

/// Import a profile — the text of a file the operator picked — onto this
/// machine: lay it over the stored settings ([`overlay_profile`]), validate,
/// persist, and return the stored value. A file that is not a profile is
/// refused with `profile_not_settings` and nothing is written.
pub async fn import_profile(pool: &SqlitePool, text: &str) -> AppResult<Settings> {
    let stored = load(pool).await?;
    let merged = overlay_profile(&stored, text)?;
    save(pool, merged).await
}

/// Lay a profile's fields over `stored`, ONE FIELD AT A TIME: each carried key
/// the file names replaces the stored value if the result still reads as
/// [`Settings`], and is skipped (the stored value kept) if it does not. So a
/// value another version wrote differently — an enum variant that no longer
/// exists — costs that field, not the import. [`MACHINE_LOCAL`] keys and keys
/// `Settings` does not have (fields an older version had, or keys that were
/// never ours) are ignored; then [`never_take_away`] has the last word.
///
/// Refused with `profile_not_settings` — the stable code the renderer
/// translates — when the text (a leading byte-order mark aside) is not a JSON
/// object, or when not one carried field in it could be read: that is not a
/// profile, whatever its name says. "At least one carried field" is
/// deliberately the whole shape check. Because a file can only change the
/// fields it names, a stricter test (a minimum count, a marker key) would buy
/// no safety, and it would refuse the hand-trimmed profile a helper sends with
/// just the schedule in it.
pub(crate) fn overlay_profile(stored: &Settings, text: &str) -> AppResult<Settings> {
    let not_settings = |why: &str| AppError::Validation(format!("profile_not_settings: {why}"));
    // Notepad on Windows writes a BOM in front of UTF-8; it is not JSON.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Ok(Value::Object(file)) = serde_json::from_str::<Value>(text) else {
        return Err(not_settings("the file is not a JSON object"));
    };
    let mut merged = serde_json::to_value(stored)?;
    let mut applied = 0usize;
    let mut skipped = Vec::new();
    for (key, value) in file {
        // `Settings` serialises every field, so the stored object's keys ARE
        // the known keys.
        if MACHINE_LOCAL.contains(&key.as_str()) || merged.get(&key).is_none() {
            continue;
        }
        if let Some((_, strict)) = STRICT.iter().find(|(k, _)| *k == key) {
            if !strict(&value) {
                skipped.push(key);
                continue;
            }
        }
        let mut trial = merged.clone();
        trial[&key] = value;
        if Settings::deserialize(&trial).is_ok() {
            merged = trial;
            applied += 1;
        } else {
            skipped.push(key);
        }
    }
    if applied == 0 {
        return Err(not_settings("no setting in the file could be read"));
    }
    if !skipped.is_empty() {
        // Field NAMES only — never a value from the file.
        tracing::warn!(fields = ?skipped, "a profile's unreadable fields were skipped; this machine's values are kept");
    }
    let mut merged = Settings::deserialize(&merged)?;
    never_take_away(stored, &mut merged);
    Ok(merged)
}

/// The carried settings an import may move only one way, because the other
/// way silently costs a recording:
///
/// - **The weekly schedule** (`slots`) and **the special recordings**: an
///   EMPTY list in the file keeps this machine's. An exported profile always
///   carries both keys, so "the file has a schedule key" would not protect the
///   blank laptop's profile. A schedule is cleared in the schedule card, where
///   the operator sees it go. (A non-empty list does replace the stored one:
///   carrying the schedule to the second machine is what the feature is for.)
/// - **«Ta opp automatisk»** (`autoRecordEnabled`) may be switched on, never
///   off: a disarmed plan is an emptied schedule by another name.
/// - **Automatic deletion** (`autoDeleteDays`, the only setting that moves
///   recordings — retention puts them in the papirkurv): see
///   [`retention_after_import`].
fn never_take_away(stored: &Settings, merged: &mut Settings) {
    if merged.slots.is_empty() {
        merged.slots = stored.slots.clone();
    }
    if merged.special_recordings.is_empty()
        || merged.special_recordings == stored.special_recordings
    {
        // Nothing (new) from the file: this machine's specials stay exactly as
        // they are, devices included — even an older entry without an `id`.
        merged.special_recordings = stored.special_recordings.clone();
    } else {
        without_special_devices(stored, merged);
    }
    merged.auto_record_enabled |= stored.auto_record_enabled;
    merged.auto_delete_days =
        retention_after_import(stored.auto_delete_days, merged.auto_delete_days);
}

/// A special recording's `deviceId` is a sound card on the machine that wrote
/// the profile (a picker id such as `BuiltInMicrophoneDevice` is the SAME on
/// every Mac, so it could match a different card here and record a concert
/// from the wrong one). The rule, deliberately the simplest that cannot hit
/// another card: a special in the file never brings a device of its own. It
/// keeps THIS machine's choice only when a stored special has the same
/// non-blank `id` (the same concert, already set up here — its device is
/// whatever the operator chose on this machine); every other special gets
/// `None` and records on the global device, which is the machine's own.
fn without_special_devices(stored: &Settings, merged: &mut Settings) {
    for special in &mut merged.special_recordings {
        special.device_id = special
            .id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .and_then(|id| {
                stored
                    .special_recordings
                    .iter()
                    .find(|own| own.id.as_deref() == Some(id))
            })
            .and_then(|own| own.device_id.clone());
    }
}

/// The automatic-deletion age (days, `0` = off) after an import: the file may
/// switch it OFF or make it LONGER, never switch it on or make it shorter.
/// Turning it on or shortening it is a decision Avansert asks a confirmation
/// for — «opptak eldre enn N dager flyttes til papirkurven» — and retention
/// acts on it within hours, on recordings the import's own question promised
/// it would not touch.
fn retention_after_import(stored: i32, from_file: i32) -> i32 {
    if stored <= 0 || from_file <= 0 {
        0
    } else {
        from_file.max(stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::schedule::{ScheduleSlot, SpecialRecording};
    use sundayrec_core::settings::{
        ChannelMode, DeviceChannels, FileFormat, SampleRate, UpdateChannel,
    };

    /// The church PC on Saturday evening: everything [`MACHINE_LOCAL`] set to
    /// something no default is, plus a schedule, a special recording, a
    /// language and the sound.
    fn church_pc() -> Settings {
        Settings {
            language: Some("sv".into()),
            onboarding_done: true,
            device_id: Some("x32-usb".into()),
            device_name: Some("X32 USB Audio".into()),
            device_channels: [(
                "x32-usb".to_string(),
                DeviceChannels {
                    channel_l: 16,
                    channel_r: 17,
                },
            )]
            .into_iter()
            .collect(),
            input_channel_l: Some(16),
            input_channel_r: Some(17),
            channels: ChannelMode::MonoL,
            sample_rate_mode: SampleRate::R48000,
            classic_directshow: true,
            classic_ffmpeg_audio: true,
            classic_ffmpeg_preroll: true,
            video_enabled: true,
            video_device_name: Some("Logitech BRIO".into()),
            video_device_index: Some(1),
            video_flip: true,
            save_folder: Some("/Volumes/Kirke/Opptak".into()),
            editor_intro_path: Some("/Users/kirke/Musikk/intro.mp3".into()),
            editor_outro_path: Some("/Users/kirke/Musikk/outro.mp3".into()),
            update_channel: UpdateChannel::Beta,
            launch_at_login: true,
            wake_from_sleep: false,
            auto_update: false,
            silence_threshold: -40,
            slots: vec![ScheduleSlot {
                days: vec![6],
                start: "11:00".into(),
                stop: "12:30".into(),
                max: None,
            }],
            special_recordings: vec![SpecialRecording {
                id: Some("konsert".into()),
                date: "2099-12-24".into(),
                name: "Julekonsert".into(),
                start: "19:00".into(),
                stop: "21:00".into(),
                device_id: None,
            }],
            publish_target: PublishTarget::Youtube,
            ..Default::default()
        }
    }

    /// The JSON a never-set-up laptop exports: every key, every default — the
    /// way a profile written BEFORE this module looked, machine-local keys
    /// included.
    fn blank_laptop_profile_with_every_key() -> String {
        serde_json::to_string_pretty(&Settings {
            auto_delete_days: 14,
            ..Default::default()
        })
        .unwrap()
    }

    fn machine_local_values(s: &Settings) -> Value {
        let all = serde_json::to_value(s).unwrap();
        MACHINE_LOCAL
            .iter()
            .map(|k| (k.to_string(), all[*k].clone()))
            .collect()
    }

    #[test]
    fn every_machine_local_key_is_a_settings_field() {
        let fields = serde_json::to_value(Settings::default()).unwrap();
        for key in MACHINE_LOCAL {
            assert!(
                fields.get(*key).is_some(),
                "`{key}` is not a Settings field"
            );
        }
        for (key, _) in STRICT {
            assert!(
                fields.get(*key).is_some(),
                "`{key}` is not a Settings field"
            );
            assert!(!MACHINE_LOCAL.contains(key), "`{key}` is both");
        }
    }

    #[test]
    fn every_settings_field_is_classified() {
        // The ratchet: a new `Settings` field is a failing test until it is
        // put in exactly one of the three lists.
        let all: std::collections::BTreeSet<String> = serde_json::to_value(Settings::default())
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut listed = Vec::new();
        for list in [MACHINE_LOCAL, SHARED, ONE_WAY] {
            listed.extend(list.iter().map(|k| k.to_string()));
        }
        let unique: std::collections::BTreeSet<String> = listed.iter().cloned().collect();
        assert_eq!(unique.len(), listed.len(), "a key is in more than one list");
        let unclassified: Vec<_> = all.difference(&unique).collect();
        let unknown: Vec<_> = unique.difference(&all).collect();
        assert!(
            unclassified.is_empty() && unknown.is_empty(),
            "add each new Settings field to MACHINE_LOCAL, SHARED or ONE_WAY. \
             unclassified: {unclassified:?}; not a Settings field: {unknown:?}"
        );
    }

    #[test]
    fn the_machine_local_line_is_pinned() {
        // The behavioural tests below read `MACHINE_LOCAL` itself, so moving a
        // key out of it (into SHARED, say) would move the tests with it. This
        // one does not: the list is spelled out, and changing the line of
        // what never travels is an edit in two places on purpose.
        let mut expected = vec![
            "deviceId",
            "deviceName",
            "deviceChannels",
            "inputChannelL",
            "inputChannelR",
            "channels",
            "sampleRateMode",
            "classicDirectshow",
            "classicFfmpegAudio",
            "classicFfmpegPreroll",
            "videoEnabled",
            "videoDeviceName",
            "videoDeviceIndex",
            "videoFlip",
            "saveFolder",
            "editorIntroPath",
            "editorOutroPath",
            "launchAtLogin",
            "wakeFromSleep",
            "onboardingDone",
            "autoUpdate",
            "updateChannel",
        ];
        let mut actual = MACHINE_LOCAL.to_vec();
        expected.sort_unstable();
        actual.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn the_church_pc_fixture_sets_every_machine_local_field_off_its_default() {
        // Without this a `MACHINE_LOCAL` entry nobody exercises can be deleted
        // and every test stays green: stored == file == default.
        let church = machine_local_values(&church_pc());
        let default = machine_local_values(&Settings::default());
        for key in MACHINE_LOCAL {
            assert_ne!(
                church[*key], default[*key],
                "`{key}` is at its default in church_pc()"
            );
        }
    }

    #[test]
    fn a_special_recordings_sound_card_is_not_carried_either_way() {
        let mut church = church_pc();
        church.special_recordings[0].device_id = Some("BuiltInMicrophoneDevice".into());
        // Out: not in the file.
        let text = profile_json(&church).unwrap();
        assert!(!text.contains("BuiltInMicrophoneDevice"), "exported");
        // In: a file that DOES carry one (a profile written before this
        // change) gives it to no special…
        let old_file = serde_json::to_string(&serde_json::json!({
            "specialRecordings": [
                { "id": "ny", "date": "2099-12-24", "name": "Julekonsert",
                  "start": "19:00", "stop": "21:00", "deviceId": "BuiltInMicrophoneDevice" },
                { "date": "2099-12-25", "name": "Uten id",
                  "start": "19:00", "stop": "21:00", "deviceId": "BuiltInMicrophoneDevice" },
                { "id": "  ", "date": "2099-12-26", "name": "Blank id",
                  "start": "19:00", "stop": "21:00", "deviceId": "BuiltInMicrophoneDevice" },
            ]
        }))
        .unwrap();
        let merged = overlay_profile(&church_pc(), &old_file).unwrap();
        assert_eq!(merged.special_recordings.len(), 3);
        for sp in &merged.special_recordings {
            assert_eq!(sp.device_id, None, "{}", sp.name);
        }
        // …except that the same concert, already set up HERE, keeps the card
        // this machine's operator chose for it — not the file's.
        let mut here = church_pc();
        here.special_recordings[0].device_id = Some("zoom-h6".into());
        let same = serde_json::to_string(&serde_json::json!({
            "specialRecordings": [
                { "id": "konsert", "date": "2099-12-24", "name": "Julekonsert",
                  "start": "19:00", "stop": "21:00", "deviceId": "BuiltInMicrophoneDevice" },
            ]
        }))
        .unwrap();
        let merged = overlay_profile(&here, &same).unwrap();
        assert_eq!(
            merged.special_recordings[0].device_id.as_deref(),
            Some("zoom-h6")
        );
        // And an empty list keeps this machine's specials, devices included.
        let kept =
            overlay_profile(&here, r#"{ "specialRecordings": [], "language": "en" }"#).unwrap();
        assert_eq!(kept.special_recordings, here.special_recordings);
        // An older special without an `id` keeps its card when the file says
        // nothing about specials, or gives an empty list.
        let mut old_here = church_pc();
        old_here.special_recordings[0].id = None;
        old_here.special_recordings[0].device_id = Some("zoom-h6".into());
        for file in [r#"{ "language": "en" }"#, r#"{ "specialRecordings": [] }"#] {
            let kept = overlay_profile(&old_here, file).unwrap();
            assert_eq!(
                kept.special_recordings, old_here.special_recordings,
                "{file}"
            );
        }
    }

    #[test]
    fn every_lenient_field_is_machine_local_or_checked_strictly() {
        // A `deserialize_with` turns garbage into a default instead of failing
        // — the overlay's per-field check cannot see it. Read the model's own
        // source so a new lenient field is a failing test until classified.
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../crates/sundayrec-core/src/settings.rs"),
        )
        .unwrap();
        let body = &src[src
            .find("pub struct Settings {")
            .expect("the Settings struct")..];
        let body = &body[..body.find("\n}\n").expect("its end")];
        let mut lenient = Vec::new();
        let mut pending = false;
        for line in body.lines() {
            let line = line.trim();
            if line.contains("deserialize_with") {
                pending = true;
            }
            if pending {
                if let Some(rest) = line.strip_prefix("pub ") {
                    let field = rest.split(':').next().unwrap().trim();
                    // snake_case → the serialised camelCase name
                    let mut camel = String::new();
                    let mut upper = false;
                    for c in field.chars() {
                        if c == '_' {
                            upper = true;
                        } else if upper {
                            camel.extend(c.to_uppercase());
                            upper = false;
                        } else {
                            camel.push(c);
                        }
                    }
                    lenient.push(camel);
                    pending = false;
                }
            }
        }
        assert!(
            lenient.len() >= 3,
            "found only {lenient:?} — the reader is broken, and would pass vacuously"
        );
        for key in &lenient {
            assert!(
                MACHINE_LOCAL.contains(&key.as_str()) || STRICT.iter().any(|(k, _)| k == key),
                "`{key}` deserialises leniently: add it to MACHINE_LOCAL or STRICT"
            );
        }
    }

    #[test]
    fn a_blank_laptops_profile_takes_nothing_from_the_church_pc() {
        // The review's scenario, the pure half (the end-to-end one through the
        // file and the database is in `commands::settings`).
        let church = church_pc();
        let merged = overlay_profile(&church, &blank_laptop_profile_with_every_key()).unwrap();
        assert_eq!(machine_local_values(&merged), machine_local_values(&church));
        assert_eq!(merged.slots, church.slots);
        assert_eq!(merged.special_recordings, church.special_recordings);
        assert_eq!(merged.auto_delete_days, 0, "retention is not switched on");
        assert!(merged.auto_record_enabled);
        // …while the church's WAY of recording does follow the file.
        assert_eq!(merged.language, None);
        assert_eq!(
            merged.silence_threshold,
            Settings::default().silence_threshold
        );
        assert_eq!(merged.publish_target, PublishTarget::default());
    }

    #[test]
    fn a_profile_with_only_machine_local_keys_is_not_a_profile() {
        let err = overlay_profile(
            &church_pc(),
            r#"{ "deviceId": "builtin", "deviceName": "MacBook Microphone", "launchAtLogin": false }"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("profile_not_settings"), "{err}");
    }

    #[test]
    fn half_a_device_group_moves_nothing() {
        // No partial-group path: an id without its name, or a name without its
        // routing, never reaches the stored device.
        let church = church_pc();
        let merged = overlay_profile(
            &church,
            r#"{ "deviceId": "builtin", "inputChannelL": 0, "language": "en" }"#,
        )
        .unwrap();
        assert_eq!(merged.device_id, church.device_id);
        assert_eq!(merged.device_name, church.device_name);
        assert_eq!(merged.input_channel_l, church.input_channel_l);
        assert_eq!(merged.device_channels, church.device_channels);
        assert_eq!(merged.language.as_deref(), Some("en"));
    }

    #[test]
    fn retention_can_be_switched_off_or_lengthened_never_on_or_shortened() {
        for (stored, file, after) in [
            (0, 14, 0),   // never switched on
            (0, 0, 0),    // off stays off
            (30, 14, 30), // never shortened
            (30, 60, 60), // longer is fine
            (30, 30, 30),
            (30, 0, 0), // switching it off is fine
        ] {
            assert_eq!(
                retention_after_import(stored, file),
                after,
                "{stored} ← {file}"
            );
            let merged = overlay_profile(
                &Settings {
                    auto_delete_days: stored,
                    ..church_pc()
                },
                &format!(r#"{{ "autoDeleteDays": {file} }}"#),
            )
            .unwrap();
            assert_eq!(
                merged.auto_delete_days, after,
                "{stored} ← {file} via the overlay"
            );
        }
    }

    #[test]
    fn the_weekly_plan_can_be_armed_never_disarmed() {
        let off = Settings {
            auto_record_enabled: false,
            ..church_pc()
        };
        let armed = overlay_profile(&off, r#"{ "autoRecordEnabled": true }"#).unwrap();
        assert!(armed.auto_record_enabled);
        let still = overlay_profile(&church_pc(), r#"{ "autoRecordEnabled": false }"#).unwrap();
        assert!(still.auto_record_enabled);
    }

    #[test]
    fn a_profile_changes_only_the_carried_fields_it_names() {
        let church = church_pc();
        let merged = overlay_profile(&church, r#"{ "format": "flac" }"#).unwrap();
        assert_eq!(
            merged,
            Settings {
                format: FileFormat::Flac,
                ..church
            }
        );
    }

    #[test]
    fn an_unreadable_field_costs_that_field_not_the_import() {
        let church = church_pc();
        let merged = overlay_profile(
            &church,
            r#"{ "format": "wma", "language": "en", "silenceThreshold": "loud" }"#,
        )
        .unwrap();
        assert_eq!(merged.format, church.format);
        assert_eq!(merged.silence_threshold, church.silence_threshold);
        assert_eq!(merged.language.as_deref(), Some("en"));
    }

    #[test]
    fn a_lenient_fields_garbage_is_skipped_and_does_not_count() {
        let church = church_pc();
        // Through the lenient deserializer this would have become SoundCloud.
        let merged =
            overlay_profile(&church, r#"{ "publishTarget": 42, "language": "en" }"#).unwrap();
        assert_eq!(merged.publish_target, PublishTarget::Youtube);
        // …and on its own it is not a profile.
        for text in [
            r#"{ "publishTarget": 42 }"#,
            r#"{ "publishTarget": "myspace" }"#,
        ] {
            let err = overlay_profile(&church, text).unwrap_err();
            assert!(
                err.to_string().contains("profile_not_settings"),
                "{text}: {err}"
            );
        }
        // A real value is taken.
        let merged = overlay_profile(&church, r#"{ "publishTarget": "spotify" }"#).unwrap();
        assert_eq!(merged.publish_target, PublishTarget::Spotify);
    }

    #[test]
    fn a_leading_byte_order_mark_is_not_the_end_of_the_profile() {
        let merged = overlay_profile(&church_pc(), "\u{feff}{ \"language\": \"da\" }").unwrap();
        assert_eq!(merged.language.as_deref(), Some("da"));
    }

    #[test]
    fn unknown_keys_are_ignored_and_a_file_of_only_unknown_keys_is_not_a_profile() {
        let church = church_pc();
        // `hasLaunched` left in v0.15; an old profile still carries it.
        let merged =
            overlay_profile(&church, r#"{ "hasLaunched": true, "language": "de" }"#).unwrap();
        assert_eq!(merged.language.as_deref(), Some("de"));
        for text in [
            r#"{ "hasLaunched": true }"#,
            "{}",
            "[]",
            "null",
            "",
            "not json",
            "\u{feff}",
        ] {
            let err = overlay_profile(&church, text).unwrap_err();
            assert!(
                err.to_string().contains("profile_not_settings"),
                "{text:?}: {err}"
            );
        }
    }

    #[test]
    fn the_export_leaves_this_machine_out() {
        let church = church_pc();
        let text = profile_json(&church).unwrap();
        let fields: Value = serde_json::from_str(&text).unwrap();
        for key in MACHINE_LOCAL {
            assert!(fields.get(*key).is_none(), "{key} was exported");
        }
        assert!(!text.contains("X32") && !text.contains("/Users/kirke"));
        assert!(fields.get("slots").is_some() && fields.get("language").is_some());
        // It still imports: everything it does carry arrives.
        let merged = overlay_profile(&Settings::default(), &text).unwrap();
        assert_eq!(merged.slots, church.slots);
        assert_eq!(merged.language, church.language);
    }
}
