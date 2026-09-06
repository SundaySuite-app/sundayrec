//! The one answer to "which language is the person at this machine reading?",
//! for the places that cannot ask the database (F1 finding A8).
//!
//! ## Why a cell and not a lookup
//!
//! Most of the shell can just read it: [`crate::notify::dispatch_failure`],
//! [`crate::scheduler`] and the relay all have a loaded
//! [`Settings`](sundayrec_core::settings::Settings) in hand, and
//! `MailLang::from_code(settings.language.as_deref())` is the honest, freshest
//! source. Those sites keep doing exactly that — this module is not for them.
//!
//! It is for the two places where a settings read is not available or not
//! allowed:
//!
//!   - **The capture loop.** `recorder::engine` and
//!     `recorder::native_capture::segment` emit their terminal failures from
//!     inside a `tokio::select!` that is draining a reader channel. A database
//!     round-trip there is the back-pressure bug of 2026-07-31 all over again,
//!     and threading a language parameter through `run_segment` would edit the
//!     hardware-verified capture path — the one thing this codebase's own rules
//!     say not to do for a reporting reason.
//!   - **`supervise::TaskAlert`.** The alert fires from a supervisor loop that
//!     has no pool, and it must say something even when the reason it is firing
//!     is that the app is unwell.
//!
//! So the language is cached: an `AtomicU8`, written whenever anybody loads the
//! settings and whenever the renderer pushes a language change, read with one
//! relaxed load. That is a cache of a fact, not a second source of truth — and
//! the fallback when nothing has written it yet is Norwegian, the same fallback
//! `MailLang::from_code(None)` and `window::ui_lang` already use.
//!
//! ## Staleness, honestly
//!
//! The value can lag by exactly one settings write: a volunteer who switches
//! the app to Polish and, in the same second, unplugs the mixer, may get one
//! Norwegian notification. [`crate::settings::load`] runs on the scheduler's
//! every supervisor pass and on every dispatch, so the window closes on its
//! own; the renderer's `tray_set_language` closes it immediately. A stale
//! language for one alert is a cost worth paying to keep a database read out of
//! the capture path.

use std::sync::atomic::{AtomicU8, Ordering};

use sundayrec_core::email::MailLang;

/// The cached UI language, as an index into [`MailLang::ALL`]. `0` is
/// `MailLang::No`, which is also the value before anything has been loaded.
static UI_LANG: AtomicU8 = AtomicU8::new(0);

/// The index `lang` occupies in [`MailLang::ALL`].
///
/// A `match` and not `ALL.iter().position(…)`: the compiler then refuses to
/// build if a language is ever added without a slot here, which is the same
/// promise [`sundayrec_core::alerts`]'s exhaustive catalog makes.
fn index_of(lang: MailLang) -> u8 {
    match lang {
        MailLang::No => 0,
        MailLang::En => 1,
        MailLang::De => 2,
        MailLang::Sv => 3,
        MailLang::Da => 4,
        MailLang::Pl => 5,
        MailLang::Fr => 6,
    }
}

/// The inverse. An index that is not a language (impossible through
/// [`set`], but the atomic is a `u8` and this is a fallback path) reads as
/// Norwegian rather than panicking — an alert must still go out.
fn from_index(i: u8) -> MailLang {
    MailLang::ALL
        .get(i as usize)
        .copied()
        .unwrap_or(MailLang::No)
}

/// Read a language out of `cell`. Separated from [`current`] so the whole
/// mapping is provable against a LOCAL cell — the same "parameterise the impure
/// leg" shape [`crate::supervise::supervise_loop`] uses for its notification.
///
/// It matters here more than it looks: `UI_LANG` is process-wide, and
/// [`crate::settings::load`] writes it from every test that touches the
/// database. A test that asserted on the global would be a flake waiting for a
/// busy machine, so the tests below own their cell and the two public functions
/// are one-liners over the static.
fn read(cell: &AtomicU8) -> MailLang {
    from_index(cell.load(Ordering::Relaxed))
}

/// Write a language into `cell`.
fn store(cell: &AtomicU8, lang: MailLang) {
    cell.store(index_of(lang), Ordering::Relaxed);
}

/// Resolve a settings code into `cell`, by the same [`MailLang::from_code`]
/// every other caller uses.
fn note_into(cell: &AtomicU8, code: Option<&str>) {
    store(cell, MailLang::from_code(code));
}

/// The language the next native notification / alert body should be written in.
pub fn current() -> MailLang {
    read(&UI_LANG)
}

/// Remember `lang` as the current UI language.
pub fn set(lang: MailLang) {
    store(&UI_LANG, lang);
}

/// Remember the language behind a settings code (`"pl"`, `None`, `"xx"` …).
///
/// Called from [`crate::settings::load`] (so any settings read warms it) and
/// from the `tray_set_language` command (so a language change lands at once).
pub fn note(code: Option<&str>) {
    note_into(&UI_LANG, code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_round_trips_through_the_index() {
        for &lang in MailLang::ALL {
            assert_eq!(from_index(index_of(lang)), lang, "{lang:?}");
        }
        // …and the indices are distinct, so two languages cannot share a slot.
        let mut seen = std::collections::HashSet::new();
        for &lang in MailLang::ALL {
            assert!(seen.insert(index_of(lang)), "{lang:?} shares an index");
        }
    }

    #[test]
    fn an_index_that_is_not_a_language_reads_as_norwegian() {
        assert_eq!(from_index(200), MailLang::No);
        assert_eq!(from_index(MailLang::ALL.len() as u8), MailLang::No);
    }

    #[test]
    fn a_fresh_cell_reads_as_norwegian() {
        // The value before anybody has loaded settings — a notification fired
        // in the first second of a launch is Norwegian, not a panic and not an
        // empty string.
        assert_eq!(read(&AtomicU8::new(0)), MailLang::No);
    }

    #[test]
    fn the_cell_remembers_every_language() {
        let cell = AtomicU8::new(0);
        for &lang in MailLang::ALL {
            store(&cell, lang);
            assert_eq!(read(&cell), lang, "{lang:?} did not survive the cell");
        }
    }

    #[test]
    fn a_settings_code_resolves_the_way_the_mails_do() {
        let cell = AtomicU8::new(0);
        for &lang in MailLang::ALL {
            note_into(&cell, Some(lang.as_code()));
            assert_eq!(read(&cell), lang, "{:?} round trip", lang.as_code());
        }
        // An unknown code and "follow the OS" both mean Norwegian, exactly like
        // `MailLang::from_code` — the fallback the whole module rests on.
        for code in [Some("xx"), Some(""), None] {
            store(&cell, MailLang::Pl);
            note_into(&cell, code);
            assert_eq!(read(&cell), MailLang::No, "{code:?}");
        }
    }
}
