//! Output-filename construction — pure, ported from the Electron
//! `src/main/recorder-utils.ts` (`sanitizeFilename`, `localDateStr`,
//! `buildFilename`).
//!
//! This was renderer/main-side string building in Electron. The scheduler
//! (Fase 5) needs it on the Rust side so a backend-triggered recording can name
//! its file without round-tripping to the webview (the window may be hidden or
//! closed when a scheduled recording fires). It's pure and reusable, so manual
//! recording can adopt it too.
//!
//! ## `church` pattern
//!
//! The Electron `church` pattern names the file after the liturgical day via
//! `shared/church-calendar.ts` (an Easter-computus + Norwegian holiday table),
//! now ported as [`crate::church_calendar`]. [`build_filename`] still accepts an
//! OPTIONAL precomputed `church_name` (an explicit override always wins); when
//! it is `None` the calendar resolves the liturgical day for the recording date.
//! Ordinary days (no known feast/holiday) fall back to the `plain` wording
//! (`"gudstjeneste"`). The fallback only affects the `church` pattern; every
//! other pattern is a faithful port.

use chrono::{Datelike, NaiveDateTime, Timelike};

use crate::settings::FilenamePattern;

/// Windows reserved device base names (case-insensitive) that can't be used as
/// filenames. Mirrors the Electron `WIN_RESERVED` regex.
const WIN_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Make `name` safe as a filename across macOS and Windows: strip path/illegal
/// characters, trim trailing dots/spaces, dodge reserved device names, and
/// never return empty. Direct port of `recorder-utils.ts` `sanitizeFilename`.
pub fn sanitize_filename(name: &str) -> String {
    let mut safe: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    safe = safe.trim().to_string();
    // Strip trailing dots/spaces (Windows disallows them).
    safe = safe.trim_end_matches(['.', ' ']).to_string();
    if WIN_RESERVED.iter().any(|r| r.eq_ignore_ascii_case(&safe)) {
        safe = format!("_{safe}");
    }
    if safe.is_empty() {
        "opptak".to_string()
    } else {
        safe
    }
}

/// `YYYY-MM-DD` from the local-wall datetime. Port of `localDateStr`.
pub fn local_date_str(dt: NaiveDateTime) -> String {
    format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day())
}

/// Inputs to [`build_filename`]. Borrowed so the caller keeps ownership.
pub struct FilenameParams<'a> {
    /// Output container/codec extension (`mp3`, `wav`, `flac`, `aac`).
    pub format: &'a str,
    /// The pattern the user selected.
    pub pattern: FilenamePattern,
    /// A user/override name (e.g. a special recording's title). When non-blank
    /// it wins over the pattern, exactly like the Electron `customName` branch.
    pub custom_name: Option<&'a str>,
    /// Precomputed liturgical day name for the `church` pattern, or `None` to
    /// fall back to the `plain` wording (see module header).
    pub church_name: Option<&'a str>,
    /// Segment timestamp suffix for split recordings (`splitTimestamp`), or
    /// `None` for the primary/only segment.
    pub split_timestamp: Option<&'a str>,
    /// The recording's start instant (local wall clock).
    pub now: NaiveDateTime,
}

/// Build the output filename (basename + extension, no directory). Direct port
/// of `recorder-utils.ts` `buildFilename`.
pub fn build_filename(p: &FilenameParams) -> String {
    let date = local_date_str(p.now);
    let ext = p.format;
    let ts = p
        .split_timestamp
        .map(|t| format!("_{t}"))
        .unwrap_or_default();

    if let Some(name) = p.custom_name {
        if !name.trim().is_empty() {
            let safe = sanitize_filename(name.trim());
            return format!("{safe}{ts}_{date}.{ext}");
        }
    }

    match p.pattern {
        FilenamePattern::Church => {
            // Explicit override wins; otherwise resolve the liturgical day from
            // the Norwegian church calendar and sanitise it; ordinary days fall
            // back to the `plain` wording.
            let resolved = p
                .church_name
                .map(|n| n.to_string())
                .or_else(|| crate::church_calendar::liturgical_day_name(p.now.date()))
                .map(|n| sanitize_filename(&n))
                .unwrap_or_else(|| "gudstjeneste".to_string());
            format!("{resolved}{ts}_{date}.{ext}")
        }
        FilenamePattern::Plain => format!("gudstjeneste{ts}_{date}.{ext}"),
        FilenamePattern::Datetime => {
            let time = format!("{:02}{:02}", p.now.hour(), p.now.minute());
            format!("{date}_{time}.{ext}")
        }
        FilenamePattern::Date => format!("{date}{ts}.{ext}"),
    }
}

/// Make `base_path` unique against a caller-supplied existence predicate so a
/// second recording on the same day NEVER overwrites the first. If `base_path`
/// doesn't already exist it is returned unchanged; otherwise `_2`, `_3`, … is
/// inserted BEFORE the extension (`rec.mp3` → `rec_2.mp3`) until `exists`
/// returns false. Paths with no extension get the suffix appended directly
/// (`rec` → `rec_2`). Pure: the `exists` closure is the only I/O seam, so the
/// collision logic is fully unit-tested.
pub fn make_unique_path(base_path: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(base_path) {
        return base_path.to_string();
    }
    // Split the path into "stem" + ".ext", parsing the extension from the
    // BASENAME only (a dotted directory must not be mistaken for an extension) —
    // the same rule `ext_of`/`build_filename` use. The dot index is relative to
    // the whole path so the directory is preserved verbatim.
    let last_sep = base_path.rfind(['/', '\\']);
    let name_start = last_sep.map(|i| i + 1).unwrap_or(0);
    let dot_in_name = base_path[name_start..]
        .rfind('.')
        .map(|rel| name_start + rel);
    let (stem, ext) = match dot_in_name {
        Some(dot) => (&base_path[..dot], &base_path[dot..]), // ext keeps its '.'
        None => (base_path, ""),
    };

    let mut n = 2;
    loop {
        let candidate = format!("{stem}_{n}{ext}");
        if !exists(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn sanitize_strips_illegal_and_reserved() {
        assert_eq!(sanitize_filename("a/b:c*?"), "a_b_c__");
        assert_eq!(sanitize_filename("  trailing.  "), "trailing");
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("com1"), "_com1");
        assert_eq!(sanitize_filename(""), "opptak");
        assert_eq!(sanitize_filename("   "), "opptak");
        assert_eq!(sanitize_filename("Julaften"), "Julaften");
    }

    #[test]
    fn date_pattern_default() {
        let p = FilenameParams {
            format: "mp3",
            pattern: FilenamePattern::Date,
            custom_name: None,
            church_name: None,
            split_timestamp: None,
            now: dt("2026-06-07 11:00"),
        };
        assert_eq!(build_filename(&p), "2026-06-07.mp3");
    }

    #[test]
    fn date_pattern_with_split_timestamp() {
        let p = FilenameParams {
            format: "wav",
            pattern: FilenamePattern::Date,
            custom_name: None,
            church_name: None,
            split_timestamp: Some("1130"),
            now: dt("2026-06-07 11:30"),
        };
        assert_eq!(build_filename(&p), "2026-06-07_1130.wav");
    }

    #[test]
    fn plain_and_datetime_patterns() {
        let base = FilenameParams {
            format: "mp3",
            pattern: FilenamePattern::Plain,
            custom_name: None,
            church_name: None,
            split_timestamp: None,
            now: dt("2026-06-07 09:05"),
        };
        assert_eq!(build_filename(&base), "gudstjeneste_2026-06-07.mp3");

        let dt_pat = FilenameParams {
            pattern: FilenamePattern::Datetime,
            ..base_like(dt("2026-06-07 09:05"))
        };
        assert_eq!(build_filename(&dt_pat), "2026-06-07_0905.mp3");
    }

    #[test]
    fn church_pattern_uses_name_or_falls_back() {
        let with_name = FilenameParams {
            format: "mp3",
            pattern: FilenamePattern::Church,
            custom_name: None,
            church_name: Some("1. søndag i advent"),
            split_timestamp: None,
            now: dt("2026-11-29 11:00"),
        };
        assert_eq!(
            build_filename(&with_name),
            "1. søndag i advent_2026-11-29.mp3"
        );

        // No precomputed name, ordinary day → falls back to the plain wording.
        let fallback = FilenameParams {
            church_name: None,
            ..base_like(dt("2026-11-29 11:00"))
        };
        let fallback = FilenameParams {
            pattern: FilenamePattern::Church,
            ..fallback
        };
        assert_eq!(build_filename(&fallback), "gudstjeneste_2026-11-29.mp3");
    }

    #[test]
    fn church_pattern_resolves_from_calendar() {
        // No explicit name on a known liturgical day → calendar resolves it.
        // 2026-04-05 is 1. påskedag (Easter Sunday 2026).
        let easter = FilenameParams {
            pattern: FilenamePattern::Church,
            ..base_like(dt("2026-04-05 11:00"))
        };
        assert_eq!(build_filename(&easter), "1. påskedag_2026-04-05.mp3");

        // 2026-12-25 is 1. juledag.
        let christmas = FilenameParams {
            pattern: FilenamePattern::Church,
            ..base_like(dt("2026-12-25 11:00"))
        };
        assert_eq!(build_filename(&christmas), "1. juledag_2026-12-25.mp3");

        // Explicit church_name still overrides the calendar.
        let override_name = FilenameParams {
            pattern: FilenamePattern::Church,
            church_name: Some("Spesial"),
            ..base_like(dt("2026-04-05 11:00"))
        };
        assert_eq!(build_filename(&override_name), "Spesial_2026-04-05.mp3");
    }

    #[test]
    fn custom_name_wins_over_pattern() {
        let p = FilenameParams {
            format: "flac",
            pattern: FilenamePattern::Date,
            custom_name: Some("Julaften gudstjeneste"),
            church_name: None,
            split_timestamp: None,
            now: dt("2026-12-24 16:00"),
        };
        assert_eq!(build_filename(&p), "Julaften gudstjeneste_2026-12-24.flac");

        // Blank custom name is ignored (falls through to the pattern).
        let blank = FilenameParams {
            custom_name: Some("   "),
            ..base_like(dt("2026-12-24 16:00"))
        };
        assert_eq!(build_filename(&blank), "2026-12-24.mp3");
    }

    #[test]
    fn make_unique_path_no_collision_returns_unchanged() {
        let taken: &[&str] = &[];
        let out = make_unique_path("/recs/a_2026-06-07.mp3", |p| taken.contains(&p));
        assert_eq!(out, "/recs/a_2026-06-07.mp3");
    }

    #[test]
    fn make_unique_path_single_collision_inserts_2() {
        let taken = ["/recs/a.mp3"];
        let out = make_unique_path("/recs/a.mp3", |p| taken.contains(&p));
        assert_eq!(out, "/recs/a_2.mp3");
    }

    #[test]
    fn make_unique_path_double_collision_inserts_3() {
        let taken = ["/recs/a.mp3", "/recs/a_2.mp3"];
        let out = make_unique_path("/recs/a.mp3", |p| taken.contains(&p));
        assert_eq!(out, "/recs/a_3.mp3");
    }

    #[test]
    fn make_unique_path_handles_no_extension() {
        let taken = ["/recs/a"];
        let out = make_unique_path("/recs/a", |p| taken.contains(&p));
        assert_eq!(out, "/recs/a_2");
    }

    #[test]
    fn make_unique_path_dotted_directory_not_mistaken_for_extension() {
        // A dot in a DIRECTORY name must not be treated as the file extension;
        // the basename here has no extension → suffix appended to the whole path.
        let taken = ["/my.recs/a"];
        let out = make_unique_path("/my.recs/a", |p| taken.contains(&p));
        assert_eq!(out, "/my.recs/a_2");
        // With an extension present, the dotted dir is still preserved.
        let taken2 = ["/my.recs/a.wav"];
        let out2 = make_unique_path("/my.recs/a.wav", |p| taken2.contains(&p));
        assert_eq!(out2, "/my.recs/a_2.wav");
    }

    /// A `Date`-pattern, mp3, no-extras params at `now` — test convenience.
    fn base_like(now: NaiveDateTime) -> FilenameParams<'static> {
        FilenameParams {
            format: "mp3",
            pattern: FilenamePattern::Date,
            custom_name: None,
            church_name: None,
            split_timestamp: None,
            now,
        }
    }
}
