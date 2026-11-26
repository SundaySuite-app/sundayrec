//! The two UI languages the backend speaks in — the native notifications'
//! catalogue ([`crate::alerts`]) and everything else the shell says without the
//! renderer's help.
//!
//! This used to be `email::MailLang`: the alert mail was the first thing in the
//! backend that needed a language, so the type was born there. The mail is gone
//! (the SMTP alerter and the SundaySuite relay were removed together); the
//! language is not, because every native notification still resolves through
//! it. Since F4 there are two: Norwegian and English.

/// One of the two UI languages SundayRec ships.
///
/// A code that is not `"en"` reads as Norwegian: blank, unknown, and the five languages that
/// were removed in F4 (`sv`, `da`, `de`, `fr`, `pl`). The backend has no OS-locale source of
/// its own, so this is only the answer until the renderer pushes the language it resolved
/// (`tray_set_language`, at startup and on every change) — and the renderer applies the system
/// rule (Norwegian system → Norwegian, otherwise English) to a stored language that no longer
/// exists, so the two agree within the first second of a launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    No,
    En,
}

impl Lang {
    /// Resolve a settings language code (`"no"`, `"en"`) to a [`Lang`], defaulting
    /// to Norwegian. Mirrors `settings.language ?? 'no'`.
    pub fn from_code(code: Option<&str>) -> Self {
        match code.unwrap_or("no") {
            "en" => Lang::En,
            _ => Lang::No,
        }
    }

    /// Both, in declaration order.
    pub const ALL: &'static [Lang] = &[Lang::No, Lang::En];

    /// The settings code this language was resolved FROM — the exact inverse of
    /// [`Self::from_code`]. Pinned by `every_language_round_trips_through_its_code`.
    pub fn as_code(self) -> &'static str {
        match self {
            Lang::No => "no",
            Lang::En => "en",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_resolves_and_defaults_to_norwegian() {
        assert_eq!(Lang::from_code(Some("en")), Lang::En);
        assert_eq!(Lang::from_code(Some("no")), Lang::No);
        assert_eq!(Lang::from_code(Some("xx")), Lang::No);
        assert_eq!(Lang::from_code(None), Lang::No);
    }

    #[test]
    fn a_removed_language_reads_as_norwegian_until_the_renderer_says_otherwise() {
        for code in ["sv", "da", "de", "fr", "pl"] {
            assert_eq!(Lang::from_code(Some(code)), Lang::No, "{code}");
        }
    }

    #[test]
    fn every_language_round_trips_through_its_code() {
        assert_eq!(Lang::ALL.len(), 2);
        for lang in Lang::ALL {
            assert_eq!(
                Lang::from_code(Some(lang.as_code())),
                *lang,
                "{lang:?} must round-trip through its own code"
            );
        }
        // …and the codes are the settings vocabulary, not BCP-47.
        assert_eq!(Lang::No.as_code(), "no");
    }
}
