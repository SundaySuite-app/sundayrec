//! Email alert decisions — pure, GUI-free, network-free (PU-1 P2a).
//!
//! Ported from the Electron `src/main/mailer.ts` (the behavioural spec). That
//! file interleaved the *content* (7-language localized subject/body templates)
//! with the actual sending (`nodemailer` SMTP transport). We keep ONLY the
//! deterministic decisions here:
//!   - which localized template strings to use ([`MailLang`], [`error_strings`])
//!   - rendering an error/test email to plaintext + HTML ([`render_error`],
//!     [`render_test`])
//!   - the recipient/throttle/dedup gate ([`AlertGate`]) — Electron sent on
//!     every failure; this adds a small de-dup so a flapping recorder can't spam
//!     the responsible person
//!
//! (The Gmail-API raw-message assembly that sat beside these left with the
//! cloud-backup OAuth client; SMTP is the one transport.)
//!
//! The `src-tauri` shell (behind the `email` feature, in `default`) owns the
//! impure half: the SMTP socket. It calls these functions to decide *whether*
//! to send and *what* to send, then performs the single side effect.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ─────────────────────────────────────────────────────────────────────────────
//   UI-facing DTOs (the renderer's email panel)
// ─────────────────────────────────────────────────────────────────────────────

/// What the email panel needs to render itself without a failed send: whether
/// this build compiled the `email` feature in at all. Filled by the
/// `src-tauri` shell from the cargo feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "EmailStatus.ts")]
#[serde(rename_all = "camelCase")]
pub struct EmailStatus {
    /// True when the binary was built with `--features email` (the send path is
    /// present). A `--no-default-features` build is `false` → the panel shows a
    /// calm hint.
    pub feature_built: bool,
}

/// The seven UI languages SundayRec ships, matching `mailer.ts` `MAIL_STRINGS`.
/// Unknown/blank language codes fall back to Norwegian (the Electron default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailLang {
    No,
    En,
    De,
    Sv,
    Da,
    Pl,
    Fr,
}

impl MailLang {
    /// Resolve a settings language code (`"no"`, `"en"`, …) to a [`MailLang`],
    /// defaulting to Norwegian. Mirrors `settings.language ?? 'no'`.
    pub fn from_code(code: Option<&str>) -> Self {
        match code.unwrap_or("no") {
            "en" => MailLang::En,
            "de" => MailLang::De,
            "sv" => MailLang::Sv,
            "da" => MailLang::Da,
            "pl" => MailLang::Pl,
            "fr" => MailLang::Fr,
            _ => MailLang::No,
        }
    }

    /// The BCP-47 locale used to format the human date (mirrors `LOCALE_MAP`).
    pub fn locale(self) -> &'static str {
        match self {
            MailLang::No => "nb-NO",
            MailLang::En => "en-GB",
            MailLang::De => "de-DE",
            MailLang::Sv => "sv-SE",
            MailLang::Da => "da-DK",
            MailLang::Pl => "pl-PL",
            MailLang::Fr => "fr-FR",
        }
    }
}

/// The localized building blocks of an error alert. Mirrors `mailer.ts`
/// `MailStrings`; the `subject` / `greeting` / `intro` are templates the caller
/// fills with church/date/person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorStrings {
    /// `{church}` + `{date}` placeholders.
    pub subject: &'static str,
    /// `{name}` placeholder.
    pub greeting: &'static str,
    /// `{church}` placeholder.
    pub intro: &'static str,
    pub error_label: &'static str,
    pub date_label: &'static str,
    pub instruction: &'static str,
    pub signoff: &'static str,
}

/// The localized strings for a test ("email works") message (mirrors `TEST_STRINGS`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestStrings {
    pub subject: &'static str,
    pub body: &'static str,
}

/// The localized error-alert strings for `lang`. Byte-for-byte ported from
/// `mailer.ts` `MAIL_STRINGS` so existing recipients see the identical wording.
pub fn error_strings(lang: MailLang) -> ErrorStrings {
    match lang {
        MailLang::No => ErrorStrings {
            subject: "⚠️ Opptaksfeil — {church} — {date}",
            greeting: "Hei {name},",
            intro: "Det oppstod en feil under planlagt opptak hos {church}:",
            error_label: "Feil",
            date_label: "Dato",
            instruction: "Vennligst sjekk at lydmikseren er koblet til og prøv et manuelt opptak.",
            signoff: "Hilsen SundayRec",
        },
        MailLang::En => ErrorStrings {
            subject: "⚠️ Recording error — {church} — {date}",
            greeting: "Hello {name},",
            intro: "An error occurred during the scheduled recording at {church}:",
            error_label: "Error",
            date_label: "Date",
            instruction: "Please check that the audio mixer is connected and try a manual recording.",
            signoff: "Regards, SundayRec",
        },
        MailLang::De => ErrorStrings {
            subject: "⚠️ Aufnahmefehler — {church} — {date}",
            greeting: "Hallo {name},",
            intro: "Bei der geplanten Aufnahme in {church} ist ein Fehler aufgetreten:",
            error_label: "Fehler",
            date_label: "Datum",
            instruction: "Bitte prüfen Sie, ob das Audiomischpult angeschlossen ist, und versuchen Sie eine manuelle Aufnahme.",
            signoff: "Mit freundlichen Grüßen, SundayRec",
        },
        MailLang::Sv => ErrorStrings {
            subject: "⚠️ Inspelningsfel — {church} — {date}",
            greeting: "Hej {name},",
            intro: "Ett fel uppstod vid den schemalagda inspelningen hos {church}:",
            error_label: "Fel",
            date_label: "Datum",
            instruction: "Kontrollera att ljudmixern är ansluten och försök med en manuell inspelning.",
            signoff: "Vänliga hälsningar, SundayRec",
        },
        MailLang::Da => ErrorStrings {
            subject: "⚠️ Optagelsesfejl — {church} — {date}",
            greeting: "Hej {name},",
            intro: "Der opstod en fejl under den planlagte optagelse hos {church}:",
            error_label: "Fejl",
            date_label: "Dato",
            instruction: "Kontroller venligst at lydmixeren er tilsluttet og prøv en manuel optagelse.",
            signoff: "Venlig hilsen, SundayRec",
        },
        MailLang::Pl => ErrorStrings {
            subject: "⚠️ Błąd nagrywania — {church} — {date}",
            greeting: "Witaj {name},",
            intro: "Wystąpił błąd podczas zaplanowanego nagrania w {church}:",
            error_label: "Błąd",
            date_label: "Data",
            instruction: "Sprawdź, czy mikser audio jest podłączony i spróbuj nagrać ręcznie.",
            signoff: "Pozdrowienia, SundayRec",
        },
        MailLang::Fr => ErrorStrings {
            subject: "⚠️ Erreur d'enregistrement — {church} — {date}",
            greeting: "Bonjour {name},",
            intro: "Une erreur s'est produite lors de l'enregistrement planifié à {church} :",
            error_label: "Erreur",
            date_label: "Date",
            instruction: "Veuillez vérifier que la console audio est connectée et essayez un enregistrement manuel.",
            signoff: "Cordialement, SundayRec",
        },
    }
}

/// The localized test-message strings for `lang` (ported from `TEST_STRINGS`).
pub fn test_strings(lang: MailLang) -> TestStrings {
    match lang {
        MailLang::No => TestStrings {
            subject: "✓ SundayRec — e-post fungerer",
            body: "E-postkonfigurasjonen er korrekt. Dette er en testmelding fra SundayRec.",
        },
        MailLang::En => TestStrings {
            subject: "✓ SundayRec — email works",
            body: "Email configuration is correct. This is a test message from SundayRec.",
        },
        MailLang::De => TestStrings {
            subject: "✓ SundayRec — E-Mail funktioniert",
            body:
                "Die E-Mail-Konfiguration ist korrekt. Dies ist eine Testnachricht von SundayRec.",
        },
        MailLang::Sv => TestStrings {
            subject: "✓ SundayRec — e-post fungerar",
            body: "E-postkonfigurationen är korrekt. Detta är ett testmeddelande från SundayRec.",
        },
        MailLang::Da => TestStrings {
            subject: "✓ SundayRec — e-mail virker",
            body: "E-mailkonfigurationen er korrekt. Dette er en testbesked fra SundayRec.",
        },
        MailLang::Pl => TestStrings {
            subject: "✓ SundayRec — e-mail działa",
            body: "Konfiguracja e-mail jest poprawna. To jest wiadomość testowa z SundayRec.",
        },
        MailLang::Fr => TestStrings {
            subject: "✓ SundayRec — e-mail fonctionne",
            body: "La configuration e-mail est correcte. C'est un message test de SundayRec.",
        },
    }
}

/// A rendered email: localized subject + plaintext + HTML bodies, ready for the
/// shell to wrap in an SMTP message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedEmail {
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// HTML-escape (mirrors `mailer.ts` `esc`): `& < > " '` → entities.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Fill `{placeholder}` tokens from `vars`. Unknown placeholders are left as-is,
/// matching the JS template-literal behaviour for the fields we control.
fn fill(template: &str, vars: &HashMap<&str, &str>) -> String {
    let mut out = template.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// Render a recording-error alert to subject + plaintext + HTML, exactly as
/// `mailer.ts` `sendError` builds them. `date` is the already-localized human
/// date string (the shell formats it with [`MailLang::locale`] — date
/// formatting is a wall-clock/ICU concern that stays out of the pure core).
/// `church` defaults to "SundayRec" when blank, matching the Electron fallback.
pub fn render_error(
    lang: MailLang,
    church: &str,
    person: &str,
    date: &str,
    error_message: &str,
) -> RenderedEmail {
    let s = error_strings(lang);
    let church = if church.trim().is_empty() {
        "SundayRec"
    } else {
        church
    };

    let mut vars = HashMap::new();
    vars.insert("church", church);
    vars.insert("date", date);
    let subject = fill(s.subject, &vars);

    let mut gvars = HashMap::new();
    gvars.insert("name", person);
    let greeting = fill(s.greeting, &gvars);

    let mut ivars = HashMap::new();
    ivars.insert("church", church);
    let intro = fill(s.intro, &ivars);

    let text = [
        greeting.as_str(),
        "",
        intro.as_str(),
        "",
        &format!("{}: {}", s.error_label, error_message),
        &format!("{}: {}", s.date_label, date),
        "",
        s.instruction,
        "",
        s.signoff,
    ]
    .join("\n");

    let html = format!(
        "\n    <p>{}</p>\n    <p>{}</p>\n    <blockquote style=\"background:#fee;padding:12px;border-left:4px solid #f05;\">\n      <strong>{}:</strong> {}<br>\n      <strong>{}:</strong> {}\n    </blockquote>\n    <p>{}</p>\n    <p>{}</p>\n  ",
        esc(&greeting),
        esc(&intro),
        esc(s.error_label),
        esc(error_message),
        esc(s.date_label),
        esc(date),
        esc(s.instruction),
        esc(s.signoff),
    );

    RenderedEmail {
        subject,
        text,
        html,
    }
}

/// Render the localized "email works" test message (subject + plaintext). The
/// test mail has no HTML part in Electron, so [`RenderedEmail::html`] is empty.
pub fn render_test(lang: MailLang) -> RenderedEmail {
    let t = test_strings(lang);
    RenderedEmail {
        subject: t.subject.to_string(),
        text: t.body.to_string(),
        html: String::new(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Recipient / throttle / dedup gate
// ─────────────────────────────────────────────────────────────────────────────

/// Whether (and why not) an alert should be sent right now. Electron sent on
/// every failure with no recipient guard beyond `if (!emailAddress) return`;
/// this adds a small de-dup window so a recorder that flaps (reconnect storms)
/// can't bury the responsible person in identical alerts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertDecision {
    /// Send it. Carries the trimmed recipient.
    Send { recipient: String },
    /// No recipient configured (`emailAddress` blank) — silently skip.
    NoRecipient,
    /// An identical alert was sent within the throttle window — suppress.
    Throttled,
}

/// The minimum gap between two *identical* error alerts. A flapping recorder
/// emits the same error code repeatedly; one mail per 10 minutes is plenty.
pub const ALERT_THROTTLE_MS: i64 = 10 * 60 * 1000;

/// Pure throttle/dedup state. The shell holds one of these (in managed state)
/// and feeds the wall clock in; no timers, no I/O.
#[derive(Debug, Clone, Default)]
pub struct AlertGate {
    /// Last `(recipient, error_message)` sent → unix-ms it went out.
    last_sent: HashMap<(String, String), i64>,
}

impl AlertGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide whether to send an error alert for `error_message` to the
    /// configured `recipient` at `now_ms`. Blank recipient → [`AlertDecision::NoRecipient`];
    /// an identical `(recipient, message)` within [`ALERT_THROTTLE_MS`] →
    /// [`AlertDecision::Throttled`]; otherwise [`AlertDecision::Send`].
    ///
    /// This does NOT mutate — call [`record_sent`](Self::record_sent) only after
    /// the shell actually dispatched the mail, so a send failure doesn't start a
    /// throttle window.
    pub fn decide(&self, recipient: &str, error_message: &str, now_ms: i64) -> AlertDecision {
        let recipient = recipient.trim();
        if recipient.is_empty() {
            return AlertDecision::NoRecipient;
        }
        let key = (recipient.to_string(), error_message.to_string());
        if let Some(&last) = self.last_sent.get(&key) {
            if now_ms.saturating_sub(last) < ALERT_THROTTLE_MS {
                return AlertDecision::Throttled;
            }
        }
        AlertDecision::Send {
            recipient: recipient.to_string(),
        }
    }

    /// Record that an alert was dispatched, opening a throttle window.
    pub fn record_sent(&mut self, recipient: &str, error_message: &str, now_ms: i64) {
        self.last_sent.insert(
            (recipient.trim().to_string(), error_message.to_string()),
            now_ms,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_status_serialises_camel_case() {
        let s = EmailStatus {
            feature_built: false,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"featureBuilt\":false"));
    }

    #[test]
    fn lang_resolves_and_defaults_to_norwegian() {
        assert_eq!(MailLang::from_code(Some("en")), MailLang::En);
        assert_eq!(MailLang::from_code(Some("fr")), MailLang::Fr);
        assert_eq!(MailLang::from_code(Some("xx")), MailLang::No);
        assert_eq!(MailLang::from_code(None), MailLang::No);
        assert_eq!(MailLang::De.locale(), "de-DE");
    }

    #[test]
    fn all_seven_languages_have_complete_catalogs() {
        for lang in [
            MailLang::No,
            MailLang::En,
            MailLang::De,
            MailLang::Sv,
            MailLang::Da,
            MailLang::Pl,
            MailLang::Fr,
        ] {
            let e = error_strings(lang);
            // Subject must carry both fill-points so the church + date land.
            assert!(e.subject.contains("{church}"), "{lang:?} subject church");
            assert!(e.subject.contains("{date}"), "{lang:?} subject date");
            assert!(e.greeting.contains("{name}"), "{lang:?} greeting name");
            assert!(e.intro.contains("{church}"), "{lang:?} intro church");
            assert!(!e.instruction.is_empty());
            let t = test_strings(lang);
            assert!(!t.subject.is_empty() && !t.body.is_empty());
        }
    }

    #[test]
    fn render_error_fills_norwegian_template() {
        let r = render_error(
            MailLang::No,
            "Oslo domkirke",
            "Ola",
            "søndag 31. mai 2026",
            "ffmpeg crashed",
        );
        assert_eq!(
            r.subject,
            "⚠️ Opptaksfeil — Oslo domkirke — søndag 31. mai 2026"
        );
        assert!(r.text.starts_with("Hei Ola,\n"));
        assert!(r.text.contains("Feil: ffmpeg crashed"));
        assert!(r.text.contains("Dato: søndag 31. mai 2026"));
        assert!(r
            .text
            .contains("Det oppstod en feil under planlagt opptak hos Oslo domkirke:"));
        assert!(r.text.trim_end().ends_with("Hilsen SundayRec"));
        // HTML carries the same content, escaped.
        assert!(r.html.contains("<strong>Feil:</strong> ffmpeg crashed"));
    }

    #[test]
    fn render_error_escapes_html_in_the_error_message() {
        let r = render_error(
            MailLang::En,
            "St <Mary>",
            "A&B",
            "today",
            "<script>x</script>",
        );
        // Plaintext is raw…
        assert!(r.text.contains("Error: <script>x</script>"));
        // …HTML is escaped.
        assert!(r.html.contains("&lt;script&gt;x&lt;/script&gt;"));
        assert!(r.html.contains("St &lt;Mary&gt;"));
        assert!(!r.html.contains("<script>"));
    }

    #[test]
    fn render_error_defaults_blank_church_to_sundayrec() {
        let r = render_error(MailLang::En, "   ", "", "today", "boom");
        assert!(r.subject.contains("SundayRec"));
        assert!(r.text.contains("at SundayRec:"));
    }

    #[test]
    fn render_test_has_no_html_part() {
        let r = render_test(MailLang::No);
        assert_eq!(r.subject, "✓ SundayRec — e-post fungerer");
        assert!(r.html.is_empty());
        assert!(r.text.contains("testmelding"));
    }

    #[test]
    fn gate_skips_when_no_recipient() {
        let gate = AlertGate::new();
        assert_eq!(gate.decide("  ", "err", 0), AlertDecision::NoRecipient);
        assert_eq!(gate.decide("", "err", 0), AlertDecision::NoRecipient);
    }

    #[test]
    fn gate_sends_then_throttles_identical_alerts() {
        let mut gate = AlertGate::new();
        assert_eq!(
            gate.decide("a@b.no", "device gone", 1_000),
            AlertDecision::Send {
                recipient: "a@b.no".into()
            }
        );
        gate.record_sent("a@b.no", "device gone", 1_000);
        // Same error within the window → throttled.
        assert_eq!(
            gate.decide("a@b.no", "device gone", 1_000 + ALERT_THROTTLE_MS - 1),
            AlertDecision::Throttled
        );
        // After the window → sends again.
        assert_eq!(
            gate.decide("a@b.no", "device gone", 1_000 + ALERT_THROTTLE_MS),
            AlertDecision::Send {
                recipient: "a@b.no".into()
            }
        );
    }

    #[test]
    fn gate_does_not_throttle_a_different_error() {
        let mut gate = AlertGate::new();
        gate.record_sent("a@b.no", "err one", 0);
        // Different message → not throttled.
        assert_eq!(
            gate.decide("a@b.no", "err two", 10),
            AlertDecision::Send {
                recipient: "a@b.no".into()
            }
        );
    }

    #[test]
    fn gate_trims_recipient_in_the_send_payload() {
        let gate = AlertGate::new();
        assert_eq!(
            gate.decide("  a@b.no  ", "e", 0),
            AlertDecision::Send {
                recipient: "a@b.no".into()
            }
        );
    }
}
