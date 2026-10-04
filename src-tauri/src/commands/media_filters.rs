//! What the editor's open dialogs offer: the formats, and the names of the
//! filters in the UI language.
//!
//! Both moved here from the renderer when the dialogs did (finding A2): the
//! webview used to open the file picker itself, with `AUDIO_EXT`/`VIDEO_EXT` in
//! `api-shim.ts` and the filter names in its catalogue
//! (`app.dialog.filter.*`). Rust opens them now (`commands::editor`'s
//! `editor_open_recording`, `commands::settings`' intro/outro pickers), so the
//! lists live where they are used.
//!
//! The names are a SEVEN-LANGUAGE catalogue — the `match` makes the compiler
//! demand all seven — which is why this file, and not `commands::editor`, is the
//! one the Norwegian-in-Rust gate allows (`scripts/rust-norwegian-baseline.json`).

use sundayrec_core::lang::Lang;

/// Every extension the bundled ffmpeg demuxes, for the open dialog — broad and
/// VLC-like on purpose: the loader falls back to a full-fidelity AAC proxy for
/// anything the webview cannot decode directly.
pub const AUDIO_EXT: &[&str] = &[
    "mp3", "mp1", "mp2", "wav", "flac", "aac", "m4a", "m4b", "m4r", "ogg", "oga", "opus", "aiff",
    "aif", "wma", "mka", "ac3", "eac3", "amr", "3ga", "caf", "wv", "tta", "au", "snd", "ape",
    "dts", "mpc", "ra", "ram", "spx", "gsm",
];
pub const VIDEO_EXT: &[&str] = &[
    "mp4", "mov", "mkv", "m4v", "webm", "avi", "wmv", "ts", "mts", "m2ts", "flv", "3gp", "asf",
    "f4v",
];

/// The three filter names the open dialog shows besides «all files», in the UI
/// language: `(all supported media, audio, video)`.
pub fn media_filter_names(lang: Lang) -> (&'static str, &'static str, &'static str) {
    match lang {
        Lang::No => ("Alle støttede medier", "Lyd", "Video"),
        Lang::En => ("All supported media", "Audio", "Video"),
        Lang::De => ("Alle unterstützten Medien", "Audio", "Video"),
        Lang::Sv => ("Alla medier som stöds", "Ljud", "Video"),
        Lang::Da => ("Alle understøttede medier", "Lyd", "Video"),
        Lang::Pl => ("Wszystkie obsługiwane media", "Audio", "Wideo"),
        Lang::Fr => ("Tous les médias pris en charge", "Audio", "Vidéo"),
    }
}

/// The name of the audio-only filter, in the UI language: the intro/outro
/// picker's one named filter.
pub fn audio_filter_name(lang: Lang) -> &'static str {
    media_filter_names(lang).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_names_the_media_filters() {
        for lang in Lang::ALL {
            let (all, audio, video) = media_filter_names(*lang);
            for name in [
                all,
                audio,
                video,
                super::super::settings::all_files_name(*lang),
            ] {
                assert!(!name.trim().is_empty(), "{lang:?}");
            }
            assert_ne!(all, audio, "{lang:?}");
        }
        // The Norwegian names, as the renderer's catalogue had them.
        assert_eq!(
            media_filter_names(Lang::No),
            ("Alle støttede medier", "Lyd", "Video")
        );
        // The dialog's extensions carry no dot and no duplicates.
        let mut every: Vec<&str> = AUDIO_EXT.iter().chain(VIDEO_EXT).copied().collect();
        assert!(every.iter().all(|e| !e.starts_with('.') && !e.is_empty()));
        let before = every.len();
        every.sort();
        every.dedup();
        assert_eq!(every.len(), before, "an extension is listed twice");
    }
}
