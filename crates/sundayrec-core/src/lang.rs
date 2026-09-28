//! The seven UI languages the backend speaks in — the native notifications'
//! catalogue ([`crate::alerts`]) and everything else the shell says without the
//! renderer's help.
//!
//! This used to be `email::MailLang`: the alert mail was the first thing in the
//! backend that needed a language, so the type was born there. The mail is gone
//! (the SMTP alerter and the SundaySuite relay were removed together); the
//! language is not, because every native notification still resolves through
//! it.

/// One of the seven UI languages SundayRec ships. Unknown/blank language codes
/// fall back to Norwegian (the Electron default, and the renderer's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    No,
    En,
    De,
    Sv,
    Da,
    Pl,
    Fr,
}

impl Lang {
    /// Resolve a settings language code (`"no"`, `"en"`, …) to a [`Lang`],
    /// defaulting to Norwegian. Mirrors `settings.language ?? 'no'`.
    pub fn from_code(code: Option<&str>) -> Self {
        match code.unwrap_or("no") {
            "en" => Lang::En,
            "de" => Lang::De,
            "sv" => Lang::Sv,
            "da" => Lang::Da,
            "pl" => Lang::Pl,
            "fr" => Lang::Fr,
            _ => Lang::No,
        }
    }

    /// All seven, in declaration order.
    pub const ALL: &'static [Lang] = &[
        Lang::No,
        Lang::En,
        Lang::De,
        Lang::Sv,
        Lang::Da,
        Lang::Pl,
        Lang::Fr,
    ];

    /// The settings code this language was resolved FROM — the exact inverse of
    /// [`Self::from_code`]. Pinned by `every_language_round_trips_through_its_code`.
    pub fn as_code(self) -> &'static str {
        match self {
            Lang::No => "no",
            Lang::En => "en",
            Lang::De => "de",
            Lang::Sv => "sv",
            Lang::Da => "da",
            Lang::Pl => "pl",
            Lang::Fr => "fr",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_resolves_and_defaults_to_norwegian() {
        assert_eq!(Lang::from_code(Some("en")), Lang::En);
        assert_eq!(Lang::from_code(Some("fr")), Lang::Fr);
        assert_eq!(Lang::from_code(Some("xx")), Lang::No);
        assert_eq!(Lang::from_code(None), Lang::No);
    }

    #[test]
    fn every_language_round_trips_through_its_code() {
        assert_eq!(Lang::ALL.len(), 7);
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
