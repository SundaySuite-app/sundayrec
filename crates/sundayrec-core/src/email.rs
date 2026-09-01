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
    let church = church_or_app(church);

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

/// Render the localized "email works" test message.
///
/// The Electron original had no HTML part — `nodemailer` was happy with a
/// plaintext-only message and nobody minded. The relay is not: its validator
/// requires BOTH body parts, non-empty, on every kind (see
/// [`RelayMessage::fits`]), because a mail with no HTML alternative reads as
/// bulk to more than one large mail host and this domain's deliverability is
/// shared by every church using it. So the test message now carries the same
/// one sentence twice, in both shapes.
///
/// The SMTP path is unaffected by construction: `send_via_smtp` already
/// switched on `html.is_empty()`, and now takes the multipart branch here too.
pub fn render_test(lang: MailLang) -> RenderedEmail {
    let t = test_strings(lang);
    RenderedEmail {
        subject: t.subject.to_string(),
        text: t.body.to_string(),
        html: format!("\n    <p>{}</p>\n  ", esc(t.body)),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The relay's own messages (A1)
// ─────────────────────────────────────────────────────────────────────────────
//
//   Three mails SMTP never sent, plus the footer that makes the other three
//   legal to send from a shared domain:
//
//     - [`render_missed`]  — a scheduled recording that never happened. The app
//       has promised this mail since `settings.rs` was written and has never
//       sent one; `FailureSource::Missed` is the routing half.
//     - [`render_receipt`] — "the recording is finished", the one piece of good
//       news in the set, off by default.
//     - [`render_confirm`] — the double opt-in mail. The only one of the five
//       that is NOT sent to a confirmed subscriber, and therefore the only one
//       without an unsubscribe footer: there is nothing yet to unsubscribe from,
//       and doing nothing is already the way to decline.
//     - [`render_unsubscribe_footer`] — appended to every other kind.
//
//   These are Rust strings, not renderer locale keys, for the same reason
//   `error_strings` is: the mail is composed in a backend that may be running
//   while no window exists, and its wording must not depend on a JSON catalog
//   the renderer owns.
//
//   ## The one rule every free-text input obeys
//
//   Everything interpolated here — church, person, error message, slot label —
//   must already have been through [`crate::telemetry::sanitize_free_text`] at
//   the RELAY call site. The rendering does not scrub (the SMTP path shares
//   `render_error` and wants the operator's full text); it only promises not to
//   RE-introduce a path, which `relay_bodies_pass_the_endpoints_own_validator`
//   proves against a copy of the endpoint's own regex. The file name in a
//   receipt is the exception: [`render_receipt`] reduces it to its basename
//   itself, because a folder name is never wanted in that field by any caller.

/// One scheduled occurrence that came and went unrecorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissedOccurrence<'a> {
    /// The schedule's own name for the slot ("Søndag formiddag").
    pub label: &'a str,
    /// The already-localized human date/time it should have started
    /// (`crate::notify::alert_date_format` formats it).
    pub date: &'a str,
}

/// The localized building blocks of a "we missed one" alert.
///
/// Two subject and two intro templates rather than one with a plural rule: the
/// count is a number in a sentence, and Norwegian, German, Polish and French
/// each inflect it differently. Two written-out strings per language say exactly
/// what the language wants (Polish sidesteps its three-way plural entirely by
/// putting the count after the noun), and a plural engine in a mail template
/// would be machinery in service of one integer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissedStrings {
    /// `{church}` + `{date}` placeholders.
    pub subject_one: &'static str,
    /// `{count}` + `{church}` + `{date}` (the OLDEST) placeholders.
    pub subject_many: &'static str,
    /// `{name}` placeholder.
    pub greeting: &'static str,
    /// `{church}` placeholder.
    pub intro_one: &'static str,
    /// `{count}` + `{church}` placeholders.
    pub intro_many: &'static str,
    /// Heading above the bullet list of occurrences.
    pub list_label: &'static str,
    pub instruction: &'static str,
    pub signoff: &'static str,
}

/// The localized "we missed one" strings for `lang`.
pub fn missed_strings(lang: MailLang) -> MissedStrings {
    match lang {
        MailLang::No => MissedStrings {
            subject_one: "⚠️ Planlagt opptak ble ikke gjort — {church} — {date}",
            subject_many: "⚠️ {count} planlagte opptak ble ikke gjort — {church} — eldste {date}",
            greeting: "Hei {name},",
            intro_one: "Et planlagt opptak hos {church} ble ikke gjort. Maskinen sov, eller SundayRec var ikke i gang da klokka passerte.",
            intro_many: "{count} planlagte opptak hos {church} ble ikke gjort. Maskinen sov, eller SundayRec var ikke i gang da klokka passerte.",
            list_label: "Dette gjelder",
            instruction: "Sjekk at maskinen står på og at SundayRec kjører før neste planlagte opptak.",
            signoff: "Hilsen SundayRec",
        },
        MailLang::En => MissedStrings {
            subject_one: "⚠️ Scheduled recording missed — {church} — {date}",
            subject_many: "⚠️ {count} scheduled recordings missed — {church} — oldest {date}",
            greeting: "Hello {name},",
            intro_one: "A scheduled recording at {church} was never made. The machine was asleep, or SundayRec was not running when the time came.",
            intro_many: "{count} scheduled recordings at {church} were never made. The machine was asleep, or SundayRec was not running when the time came.",
            list_label: "This concerns",
            instruction: "Please check that the machine is switched on and SundayRec is running before the next scheduled recording.",
            signoff: "Regards, SundayRec",
        },
        MailLang::De => MissedStrings {
            subject_one: "⚠️ Geplante Aufnahme nicht erfolgt — {church} — {date}",
            subject_many: "⚠️ {count} geplante Aufnahmen nicht erfolgt — {church} — älteste {date}",
            greeting: "Hallo {name},",
            intro_one: "Eine geplante Aufnahme in {church} wurde nicht gemacht. Der Rechner war im Ruhezustand, oder SundayRec lief zum Zeitpunkt nicht.",
            intro_many: "{count} geplante Aufnahmen in {church} wurden nicht gemacht. Der Rechner war im Ruhezustand, oder SundayRec lief zum Zeitpunkt nicht.",
            list_label: "Betroffen",
            instruction: "Bitte prüfen Sie, ob der Rechner eingeschaltet ist und SundayRec läuft, bevor die nächste geplante Aufnahme ansteht.",
            signoff: "Mit freundlichen Grüßen, SundayRec",
        },
        MailLang::Sv => MissedStrings {
            subject_one: "⚠️ Schemalagd inspelning uteblev — {church} — {date}",
            subject_many: "⚠️ {count} schemalagda inspelningar uteblev — {church} — äldsta {date}",
            greeting: "Hej {name},",
            intro_one: "En schemalagd inspelning hos {church} blev aldrig gjord. Datorn sov, eller så kördes inte SundayRec när tiden var inne.",
            intro_many: "{count} schemalagda inspelningar hos {church} blev aldrig gjorda. Datorn sov, eller så kördes inte SundayRec när tiden var inne.",
            list_label: "Det gäller",
            instruction: "Kontrollera att datorn är påslagen och att SundayRec körs inför nästa schemalagda inspelning.",
            signoff: "Vänliga hälsningar, SundayRec",
        },
        MailLang::Da => MissedStrings {
            subject_one: "⚠️ Planlagt optagelse blev ikke lavet — {church} — {date}",
            subject_many: "⚠️ {count} planlagte optagelser blev ikke lavet — {church} — ældste {date}",
            greeting: "Hej {name},",
            intro_one: "En planlagt optagelse hos {church} blev aldrig lavet. Maskinen sov, eller SundayRec kørte ikke, da tiden kom.",
            intro_many: "{count} planlagte optagelser hos {church} blev aldrig lavet. Maskinen sov, eller SundayRec kørte ikke, da tiden kom.",
            list_label: "Det drejer sig om",
            instruction: "Kontroller venligst at maskinen er tændt, og at SundayRec kører inden næste planlagte optagelse.",
            signoff: "Venlig hilsen, SundayRec",
        },
        MailLang::Pl => MissedStrings {
            // Polish has three plural forms for a bare count; putting the number
            // AFTER the noun ("nagrań: 5") is idiomatic and inflects for none of
            // them, which is why this one reads differently from its neighbours.
            subject_one: "⚠️ Zaplanowane nagranie nie zostało wykonane — {church} — {date}",
            subject_many: "⚠️ Nie wykonano zaplanowanych nagrań: {count} — {church} — najstarsze {date}",
            greeting: "Witaj {name},",
            intro_one: "Zaplanowane nagranie w {church} nie zostało wykonane. Komputer był uśpiony lub SundayRec nie był uruchomiony o wyznaczonej godzinie.",
            intro_many: "Nie wykonano zaplanowanych nagrań w {church}: {count}. Komputer był uśpiony lub SundayRec nie był uruchomiony o wyznaczonej godzinie.",
            list_label: "Dotyczy",
            instruction: "Sprawdź, czy komputer jest włączony, a SundayRec uruchomiony przed kolejnym zaplanowanym nagraniem.",
            signoff: "Pozdrowienia, SundayRec",
        },
        MailLang::Fr => MissedStrings {
            subject_one: "⚠️ Enregistrement planifié manqué — {church} — {date}",
            subject_many: "⚠️ {count} enregistrements planifiés manqués — {church} — le plus ancien {date}",
            greeting: "Bonjour {name},",
            intro_one: "Un enregistrement planifié à {church} n'a pas eu lieu. L'ordinateur était en veille, ou SundayRec n'était pas lancé à l'heure prévue.",
            intro_many: "{count} enregistrements planifiés à {church} n'ont pas eu lieu. L'ordinateur était en veille, ou SundayRec n'était pas lancé à l'heure prévue.",
            list_label: "Cela concerne",
            instruction: "Veuillez vérifier que l'ordinateur est allumé et que SundayRec est lancé avant le prochain enregistrement planifié.",
            signoff: "Cordialement, SundayRec",
        },
    }
}

/// Render the "a scheduled recording was never made" alert.
///
/// ONE mail for however many occurrences the sweep found, headlined by the
/// OLDEST — `missed[0]`, which the caller orders. `check_missed` runs at startup
/// and after every wake, so a machine that spent a fortnight switched off has
/// two weeks of Sundays to report at once; a mail each would be a small
/// mail-bomb the volunteer never asked for.
///
/// Returns `None` for an empty slice: "nothing was missed" is not a message. The
/// caller has already filtered the occurrences it has durably reported before
/// (`notify_seen`), and that filter can legitimately empty the list.
pub fn render_missed(
    lang: MailLang,
    church: &str,
    person: &str,
    missed: &[MissedOccurrence],
) -> Option<RenderedEmail> {
    let oldest = missed.first()?;
    let s = missed_strings(lang);
    let church = church_or_app(church);
    let count = missed.len().to_string();

    let mut vars = HashMap::new();
    vars.insert("church", church);
    vars.insert("date", oldest.date);
    vars.insert("count", count.as_str());
    let subject = fill(
        if missed.len() == 1 {
            s.subject_one
        } else {
            s.subject_many
        },
        &vars,
    );
    let intro = fill(
        if missed.len() == 1 {
            s.intro_one
        } else {
            s.intro_many
        },
        &vars,
    );
    let mut gvars = HashMap::new();
    gvars.insert("name", person);
    let greeting = fill(s.greeting, &gvars);

    let lines: Vec<String> = missed
        .iter()
        .map(|m| format!("- {} — {}", m.label, m.date))
        .collect();
    let text = [
        greeting.as_str(),
        "",
        intro.as_str(),
        "",
        &format!("{}:", s.list_label),
        &lines.join("\n"),
        "",
        s.instruction,
        "",
        s.signoff,
    ]
    .join("\n");

    let items: String = missed
        .iter()
        .map(|m| format!("      <li>{} — {}</li>\n", esc(m.label), esc(m.date)))
        .collect();
    let html = format!(
        "\n    <p>{}</p>\n    <p>{}</p>\n    <p><strong>{}:</strong></p>\n    <ul>\n{items}    </ul>\n    <p>{}</p>\n    <p>{}</p>\n  ",
        esc(&greeting),
        esc(&intro),
        esc(s.list_label),
        esc(s.instruction),
        esc(s.signoff),
    );

    Some(RenderedEmail {
        subject,
        text,
        html,
    })
}

/// The localized building blocks of a finished-recording receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptStrings {
    /// `{church}` + `{date}` placeholders.
    pub subject: &'static str,
    /// `{name}` placeholder.
    pub greeting: &'static str,
    /// `{church}` placeholder.
    pub intro: &'static str,
    pub slot_label: &'static str,
    pub started_label: &'static str,
    pub duration_label: &'static str,
    pub file_label: &'static str,
    pub instruction: &'static str,
    pub signoff: &'static str,
    /// Short unit for hours in a duration ("t", "h", "Std.").
    pub hour_short: &'static str,
    /// Short unit for minutes ("min", "Min.").
    pub minute_short: &'static str,
}

/// The localized receipt strings for `lang`.
pub fn receipt_strings(lang: MailLang) -> ReceiptStrings {
    match lang {
        MailLang::No => ReceiptStrings {
            subject: "✓ Opptaket er ferdig — {church} — {date}",
            greeting: "Hei {name},",
            intro: "Det planlagte opptaket hos {church} er fullført.",
            slot_label: "Opptak",
            started_label: "Startet",
            duration_label: "Varighet",
            file_label: "Fil",
            instruction: "Filen ligger i opptaksmappa på maskinen som gjorde opptaket.",
            signoff: "Hilsen SundayRec",
            hour_short: "t",
            minute_short: "min",
        },
        MailLang::En => ReceiptStrings {
            subject: "✓ Recording finished — {church} — {date}",
            greeting: "Hello {name},",
            intro: "The scheduled recording at {church} is complete.",
            slot_label: "Recording",
            started_label: "Started",
            duration_label: "Length",
            file_label: "File",
            instruction: "The file is in the recordings folder on the machine that made it.",
            signoff: "Regards, SundayRec",
            hour_short: "h",
            minute_short: "min",
        },
        MailLang::De => ReceiptStrings {
            subject: "✓ Aufnahme abgeschlossen — {church} — {date}",
            greeting: "Hallo {name},",
            intro: "Die geplante Aufnahme in {church} ist abgeschlossen.",
            slot_label: "Aufnahme",
            started_label: "Beginn",
            duration_label: "Dauer",
            file_label: "Datei",
            instruction: "Die Datei liegt im Aufnahmeordner auf dem Rechner, der aufgenommen hat.",
            signoff: "Mit freundlichen Grüßen, SundayRec",
            hour_short: "Std.",
            minute_short: "Min.",
        },
        MailLang::Sv => ReceiptStrings {
            subject: "✓ Inspelningen är klar — {church} — {date}",
            greeting: "Hej {name},",
            intro: "Den schemalagda inspelningen hos {church} är klar.",
            slot_label: "Inspelning",
            started_label: "Startade",
            duration_label: "Längd",
            file_label: "Fil",
            instruction: "Filen ligger i inspelningsmappen på datorn som spelade in.",
            signoff: "Vänliga hälsningar, SundayRec",
            hour_short: "tim",
            minute_short: "min",
        },
        MailLang::Da => ReceiptStrings {
            subject: "✓ Optagelsen er færdig — {church} — {date}",
            greeting: "Hej {name},",
            intro: "Den planlagte optagelse hos {church} er færdig.",
            slot_label: "Optagelse",
            started_label: "Startet",
            duration_label: "Varighed",
            file_label: "Fil",
            instruction: "Filen ligger i optagelsesmappen på maskinen, der optog.",
            signoff: "Venlig hilsen, SundayRec",
            hour_short: "t",
            minute_short: "min",
        },
        MailLang::Pl => ReceiptStrings {
            subject: "✓ Nagranie zakończone — {church} — {date}",
            greeting: "Witaj {name},",
            intro: "Zaplanowane nagranie w {church} zostało zakończone.",
            slot_label: "Nagranie",
            started_label: "Rozpoczęto",
            duration_label: "Czas trwania",
            file_label: "Plik",
            instruction: "Plik znajduje się w folderze nagrań na komputerze, który nagrywał.",
            signoff: "Pozdrowienia, SundayRec",
            hour_short: "godz.",
            minute_short: "min",
        },
        MailLang::Fr => ReceiptStrings {
            subject: "✓ Enregistrement terminé — {church} — {date}",
            greeting: "Bonjour {name},",
            intro: "L'enregistrement planifié à {church} est terminé.",
            slot_label: "Enregistrement",
            started_label: "Début",
            duration_label: "Durée",
            file_label: "Fichier",
            instruction:
                "Le fichier se trouve dans le dossier d'enregistrements de l'ordinateur qui a enregistré.",
            signoff: "Cordialement, SundayRec",
            hour_short: "h",
            minute_short: "min",
        },
    }
}

/// A recording length as a short localized string: `1 t 23 min`, `23 min`,
/// `1 Std.`. Minutes are floored — a receipt says how long the service was, not
/// how many seconds the encoder ran.
pub fn format_duration(lang: MailLang, secs: u64) -> String {
    let s = receipt_strings(lang);
    let total_min = secs / 60;
    let (h, m) = (total_min / 60, total_min % 60);
    match (h, m) {
        (0, m) => format!("{m} {}", s.minute_short),
        (h, 0) => format!("{h} {}", s.hour_short),
        (h, m) => format!("{h} {} {m} {}", s.hour_short, s.minute_short),
    }
}

/// Render the "your scheduled recording is finished" receipt.
///
/// `file` is reduced to its BASENAME here rather than at the call site. The
/// promise the privacy chapter makes is that the save folder stays structurally
/// outside the service, and insertion-site hygiene is how
/// [`crate::telemetry::telemetry_path`] keeps the same promise for crash
/// reports: a value that is never assembled cannot leak. Passing a full path in
/// is therefore harmless — the folder is dropped, and the endpoint's own path
/// validator (mirrored in this module's tests) would have refused the mail
/// anyway, permanently, without a retry.
pub fn render_receipt(
    lang: MailLang,
    church: &str,
    person: &str,
    slot: &str,
    started: &str,
    duration_secs: u64,
    file: &str,
) -> RenderedEmail {
    let s = receipt_strings(lang);
    let church = church_or_app(church);
    let name = basename(file);
    let duration = format_duration(lang, duration_secs);

    let mut vars = HashMap::new();
    vars.insert("church", church);
    vars.insert("date", started);
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
        &format!("{}: {}", s.slot_label, slot),
        &format!("{}: {}", s.started_label, started),
        &format!("{}: {}", s.duration_label, duration),
        &format!("{}: {}", s.file_label, name),
        "",
        s.instruction,
        "",
        s.signoff,
    ]
    .join("\n");

    let html = format!(
        "\n    <p>{}</p>\n    <p>{}</p>\n    <table role=\"presentation\">\n      <tr><td><strong>{}:</strong></td><td>{}</td></tr>\n      <tr><td><strong>{}:</strong></td><td>{}</td></tr>\n      <tr><td><strong>{}:</strong></td><td>{}</td></tr>\n      <tr><td><strong>{}:</strong></td><td>{}</td></tr>\n    </table>\n    <p>{}</p>\n    <p>{}</p>\n  ",
        esc(&greeting),
        esc(&intro),
        esc(s.slot_label),
        esc(slot),
        esc(s.started_label),
        esc(started),
        esc(s.duration_label),
        esc(&duration),
        esc(s.file_label),
        esc(name),
        esc(s.instruction),
        esc(s.signoff),
    );

    RenderedEmail {
        subject,
        text,
        html,
    }
}

/// The localized building blocks of the double opt-in confirmation mail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmStrings {
    pub subject: &'static str,
    /// `{name}` placeholder.
    pub greeting: &'static str,
    /// `{church}` placeholder.
    pub intro: &'static str,
    /// What the subscriber has signed up to receive.
    pub what: &'static str,
    /// The button's own words.
    pub button: &'static str,
    /// Introduces the bare URL, for a client that eats the button.
    pub link_hint: &'static str,
    /// How long the link lives — see [`crate::relay::CONFIRM_LINK_TTL_DAYS`].
    pub expiry: &'static str,
    /// The line that makes an unrequested mail harmless.
    pub ignore: &'static str,
    pub signoff: &'static str,
}

/// The localized confirmation-mail strings for `lang`.
pub fn confirm_strings(lang: MailLang) -> ConfirmStrings {
    match lang {
        MailLang::No => ConfirmStrings {
            subject: "Bekreft e-postvarsler fra SundayRec",
            greeting: "Hei {name},",
            intro: "Denne adressen er meldt på e-postvarsler fra SundayRec for {church}.",
            what: "Du får e-post når et opptak feiler, når et planlagt opptak ikke blir gjort, og — hvis kvitteringer er slått på — når et planlagt opptak er ferdig.",
            button: "Bekreft e-postadressen",
            link_hint: "Virker ikke knappen? Kopier denne lenka inn i nettleseren:",
            expiry: "Lenka virker i 7 dager.",
            ignore: "Har du ikke bedt om dette, kan du se bort fra denne e-posten. Vi sender ingenting før noen har bekreftet.",
            signoff: "Hilsen SundayRec",
        },
        MailLang::En => ConfirmStrings {
            subject: "Confirm e-mail alerts from SundayRec",
            greeting: "Hello {name},",
            intro: "This address has been signed up for e-mail alerts from SundayRec for {church}.",
            what: "You will get an e-mail when a recording fails, when a scheduled recording is missed, and — if receipts are switched on — when a scheduled recording finishes.",
            button: "Confirm this address",
            link_hint: "Button not working? Copy this link into your browser:",
            expiry: "The link works for 7 days.",
            ignore: "If you did not ask for this, you can ignore this e-mail. We send nothing until somebody confirms.",
            signoff: "Regards, SundayRec",
        },
        MailLang::De => ConfirmStrings {
            subject: "E-Mail-Benachrichtigungen von SundayRec bestätigen",
            greeting: "Hallo {name},",
            intro: "Diese Adresse wurde für E-Mail-Benachrichtigungen von SundayRec für {church} angemeldet.",
            what: "Sie erhalten eine E-Mail, wenn eine Aufnahme fehlschlägt, wenn eine geplante Aufnahme ausfällt und — falls Bestätigungen aktiviert sind — wenn eine geplante Aufnahme fertig ist.",
            button: "Adresse bestätigen",
            link_hint: "Funktioniert die Schaltfläche nicht? Kopieren Sie diesen Link in Ihren Browser:",
            expiry: "Der Link ist 7 Tage gültig.",
            ignore: "Wenn Sie das nicht angefordert haben, können Sie diese E-Mail ignorieren. Wir senden nichts, bevor jemand bestätigt hat.",
            signoff: "Mit freundlichen Grüßen, SundayRec",
        },
        MailLang::Sv => ConfirmStrings {
            subject: "Bekräfta e-postaviseringar från SundayRec",
            greeting: "Hej {name},",
            intro: "Den här adressen har anmälts till e-postaviseringar från SundayRec för {church}.",
            what: "Du får e-post när en inspelning misslyckas, när en schemalagd inspelning uteblir och — om kvitton är påslagna — när en schemalagd inspelning är klar.",
            button: "Bekräfta e-postadressen",
            link_hint: "Fungerar inte knappen? Kopiera den här länken till webbläsaren:",
            expiry: "Länken fungerar i 7 dagar.",
            ignore: "Har du inte bett om det här kan du bortse från meddelandet. Vi skickar ingenting förrän någon har bekräftat.",
            signoff: "Vänliga hälsningar, SundayRec",
        },
        MailLang::Da => ConfirmStrings {
            subject: "Bekræft e-mailvarsler fra SundayRec",
            greeting: "Hej {name},",
            intro: "Denne adresse er tilmeldt e-mailvarsler fra SundayRec for {church}.",
            what: "Du får en e-mail, når en optagelse fejler, når en planlagt optagelse ikke bliver lavet, og — hvis kvitteringer er slået til — når en planlagt optagelse er færdig.",
            button: "Bekræft e-mailadressen",
            link_hint: "Virker knappen ikke? Kopier dette link ind i browseren:",
            expiry: "Linket virker i 7 dage.",
            ignore: "Har du ikke bedt om dette, kan du se bort fra denne e-mail. Vi sender ikke noget, før nogen har bekræftet.",
            signoff: "Venlig hilsen, SundayRec",
        },
        MailLang::Pl => ConfirmStrings {
            subject: "Potwierdź powiadomienia e-mail z SundayRec",
            greeting: "Witaj {name},",
            intro: "Ten adres został zgłoszony do powiadomień e-mail z SundayRec dla {church}.",
            what: "Otrzymasz e-mail, gdy nagranie się nie powiedzie, gdy zaplanowane nagranie nie zostanie wykonane oraz — jeśli potwierdzenia są włączone — gdy zaplanowane nagranie się zakończy.",
            button: "Potwierdź adres e-mail",
            link_hint: "Przycisk nie działa? Skopiuj ten link do przeglądarki:",
            expiry: "Link jest ważny przez 7 dni.",
            ignore: "Jeśli nie prosiłeś o to, zignoruj tę wiadomość. Nic nie wyślemy, dopóki ktoś nie potwierdzi.",
            signoff: "Pozdrowienia, SundayRec",
        },
        MailLang::Fr => ConfirmStrings {
            subject: "Confirmez les alertes e-mail de SundayRec",
            greeting: "Bonjour {name},",
            intro: "Cette adresse a été inscrite aux alertes e-mail de SundayRec pour {church}.",
            what: "Vous recevrez un e-mail lorsqu'un enregistrement échoue, lorsqu'un enregistrement planifié est manqué et — si les accusés sont activés — lorsqu'un enregistrement planifié est terminé.",
            button: "Confirmer l'adresse e-mail",
            link_hint: "Le bouton ne fonctionne pas ? Copiez ce lien dans votre navigateur :",
            expiry: "Le lien est valable 7 jours.",
            ignore: "Si vous n'avez pas demandé cela, ignorez cet e-mail. Nous n'enverrons rien tant que personne n'a confirmé.",
            signoff: "Cordialement, SundayRec",
        },
    }
}

/// Render the double opt-in confirmation mail.
///
/// The one mail sent to an address that has NOT agreed to hear from us, which
/// is why every line of it is shaped around that fact: it says which church
/// signed the address up, what will arrive if it is confirmed, that the link
/// expires, and that doing nothing is a complete answer. Nothing is sent to the
/// address again until the link is clicked.
///
/// The URL appears twice on purpose — once as the button, once in full as text.
/// Mail clients that strip styling, and the volunteer reading on a phone who
/// wants to see where a link goes before pressing it, both need the bare form.
pub fn render_confirm(
    lang: MailLang,
    church: &str,
    person: &str,
    confirm_url: &str,
) -> RenderedEmail {
    let s = confirm_strings(lang);
    let church = church_or_app(church);

    let mut gvars = HashMap::new();
    gvars.insert("name", person);
    let greeting = fill(s.greeting, &gvars);

    let mut ivars = HashMap::new();
    ivars.insert("church", church);
    let intro = fill(s.intro, &ivars);

    // The plaintext body says the button's own words and then the bare URL.
    // `link_hint` ("button not working?") belongs to the HTML alone — in a
    // plaintext client there is no button for it to be talking about.
    let text = [
        greeting.as_str(),
        "",
        intro.as_str(),
        s.what,
        "",
        &format!("{}:", s.button),
        confirm_url,
        "",
        s.expiry,
        s.ignore,
        "",
        s.signoff,
    ]
    .join("\n");

    let html = format!(
        "\n    <p>{}</p>\n    <p>{}</p>\n    <p>{}</p>\n    <p><a href=\"{url}\" style=\"display:inline-block;padding:12px 20px;background:#2A4E92;color:#fff;border-radius:6px;text-decoration:none;\">{}</a></p>\n    <p>{}<br><a href=\"{url}\">{url_text}</a></p>\n    <p>{}<br>{}</p>\n    <p>{}</p>\n  ",
        esc(&greeting),
        esc(&intro),
        esc(s.what),
        esc(s.button),
        esc(s.link_hint),
        esc(s.expiry),
        esc(s.ignore),
        esc(s.signoff),
        url = esc(confirm_url),
        url_text = esc(confirm_url),
    );

    RenderedEmail {
        subject: s.subject.to_string(),
        text,
        html,
    }
}

/// The localized unsubscribe footer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FooterStrings {
    /// Why this mail arrived.
    pub note: &'static str,
    /// The link's own words.
    pub link_label: &'static str,
}

/// The localized footer strings for `lang`.
pub fn footer_strings(lang: MailLang) -> FooterStrings {
    match lang {
        MailLang::No => FooterStrings {
            note: "Du får denne e-posten fordi denne adressen er meldt på varsler fra SundayRec.",
            link_label: "Meld deg av",
        },
        MailLang::En => FooterStrings {
            note: "You are receiving this e-mail because this address is signed up for alerts from SundayRec.",
            link_label: "Unsubscribe",
        },
        MailLang::De => FooterStrings {
            note: "Sie erhalten diese E-Mail, weil diese Adresse für Benachrichtigungen von SundayRec angemeldet ist.",
            link_label: "Abmelden",
        },
        MailLang::Sv => FooterStrings {
            note: "Du får det här meddelandet eftersom adressen är anmäld till aviseringar från SundayRec.",
            link_label: "Avsluta prenumerationen",
        },
        MailLang::Da => FooterStrings {
            note: "Du modtager denne e-mail, fordi adressen er tilmeldt varsler fra SundayRec.",
            link_label: "Frameld dig",
        },
        MailLang::Pl => FooterStrings {
            note: "Otrzymujesz tę wiadomość, ponieważ ten adres jest zapisany do powiadomień z SundayRec.",
            link_label: "Wypisz się",
        },
        MailLang::Fr => FooterStrings {
            note: "Vous recevez cet e-mail car cette adresse est inscrite aux alertes de SundayRec.",
            link_label: "Se désinscrire",
        },
    }
}

/// A rendered footer, in both body shapes, ready to append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFooter {
    pub text: String,
    pub html: String,
}

/// Render the unsubscribe footer.
///
/// Not decoration: a bulk-shaped mail from a shared domain with no visible way
/// out is what teaches a mail host to file the sender under junk, and the
/// domain is shared by every church on the relay. The same URL backs the
/// `List-Unsubscribe` header the endpoint sets (RFC 8058), so the two ways out —
/// the client's own button and this link — are one POST.
///
/// The plaintext half opens with a blank line and NO dash rule. `-- ` on its own
/// line is the RFC 3676 signature delimiter, and clients that honour it collapse
/// everything below — which would hide precisely the link this block exists to
/// show. A separator that can hide the way out is worse than no separator.
pub fn render_unsubscribe_footer(lang: MailLang, unsubscribe_url: &str) -> RenderedFooter {
    let s = footer_strings(lang);
    RenderedFooter {
        text: format!("\n\n{}\n{}: {unsubscribe_url}", s.note, s.link_label),
        html: format!(
            "\n    <hr>\n    <p style=\"font-size:12px;color:#666;\">{}<br><a href=\"{url}\">{}</a></p>\n  ",
            esc(s.note),
            esc(s.link_label),
            url = esc(unsubscribe_url),
        ),
    }
}

impl RenderedEmail {
    /// Append a footer to both body parts.
    fn with_footer(mut self, footer: &RenderedFooter) -> Self {
        self.text.push_str(&footer.text);
        self.html.push_str(&footer.html);
        self
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The relay wire message
// ─────────────────────────────────────────────────────────────────────────────

/// Which of the five messages this is. A CLOSED set, and the endpoint's
/// validator holds the same five words — a sixth would be a 400, which the
/// outbox drops without retrying, so the two lists are changed together and the
/// Worker goes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelayMessageKind {
    /// The double opt-in mail. The only kind sent to an unconfirmed address, and
    /// the only one without an unsubscribe footer.
    Confirm,
    /// A recording failed.
    Failure,
    /// A scheduled recording never happened.
    Missed,
    /// A scheduled recording finished.
    Receipt,
    /// "E-mail works" — the button in the settings panel.
    Test,
}

impl RelayMessageKind {
    /// The wire word. Same string the endpoint's `KINDS` list holds.
    pub fn as_str(self) -> &'static str {
        match self {
            RelayMessageKind::Confirm => "confirm",
            RelayMessageKind::Failure => "failure",
            RelayMessageKind::Missed => "missed",
            RelayMessageKind::Receipt => "receipt",
            RelayMessageKind::Test => "test",
        }
    }

    /// All five, in declaration order.
    pub const ALL: &'static [RelayMessageKind] = &[
        RelayMessageKind::Confirm,
        RelayMessageKind::Failure,
        RelayMessageKind::Missed,
        RelayMessageKind::Receipt,
        RelayMessageKind::Test,
    ];

    /// Whether this kind carries the unsubscribe footer.
    ///
    /// Everything but [`Self::Confirm`], and the rule is stated as "not confirm"
    /// rather than listing four kinds so that a sixth kind gets the footer by
    /// default. The one mail that must not carry it is the one where there is
    /// no subscription yet to leave — offering an unsubscribe link to somebody
    /// who has not subscribed is at best confusing and at worst an invitation
    /// to click something in a mail they did not ask for.
    pub fn wants_unsubscribe_footer(self) -> bool {
        self != RelayMessageKind::Confirm
    }
}

/// The endpoint's subject cap, in CHARACTERS (it counts `[...str].length`, i.e.
/// UTF-16 code units, and every character below the astral plane counts as one
/// in both; the templates here hold no emoji outside the BMP).
pub const SUBJECT_MAX_CHARS: usize = 200;

/// The endpoint's plaintext-body cap, in characters.
pub const TEXT_MAX_CHARS: usize = 8_000;

/// The endpoint's HTML-body cap, in characters. Three times the text cap: the
/// same words plus markup.
pub const HTML_MAX_CHARS: usize = 24_000;

/// A message ready for the relay: the rendered mail plus which kind it is.
///
/// The client renders and the endpoint forwards — it has no template table, no
/// error-code vocabulary and no opinion about wording. What it DOES have is a
/// validator over shape and size, and this struct exists so the client can ask
/// the same questions locally, before a row is queued, rather than learning the
/// answer as a 400 that permanently drops the alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayMessage {
    pub kind: RelayMessageKind,
    pub subject: String,
    pub text: String,
    pub html: String,
}

impl RelayMessage {
    /// Wrap a rendered mail for the relay, appending the unsubscribe footer for
    /// every kind that takes one.
    ///
    /// The footer decision lives HERE, in one place, instead of in each
    /// renderer: "which mails carry an unsubscribe link" is a property of the
    /// KIND, and a rule stated once cannot be half-applied by a later call site
    /// that forgot. `unsubscribe_url` is ignored for
    /// [`RelayMessageKind::Confirm`].
    pub fn new(
        kind: RelayMessageKind,
        lang: MailLang,
        rendered: RenderedEmail,
        unsubscribe_url: &str,
    ) -> Self {
        let rendered = if kind.wants_unsubscribe_footer() {
            rendered.with_footer(&render_unsubscribe_footer(lang, unsubscribe_url))
        } else {
            rendered
        };
        Self {
            kind,
            subject: rendered.subject,
            text: rendered.text,
            html: rendered.html,
        }
    }

    /// Whether the endpoint's size and presence rules accept this message.
    ///
    /// Both body parts are required and must be non-empty — see
    /// [`render_test`], which grew an HTML part for exactly this reason — and
    /// all three fields are capped. A message that does not fit is a bug in the
    /// renderer or an absurd input, not a transient condition: the caller logs
    /// it and does not queue it, because queueing it would spend the outbox's
    /// six attempts learning what this function already knows.
    pub fn fits(&self) -> bool {
        let ok = |s: &str, max: usize| !s.is_empty() && s.chars().count() <= max;
        ok(&self.subject, SUBJECT_MAX_CHARS)
            && ok(&self.text, TEXT_MAX_CHARS)
            && ok(&self.html, HTML_MAX_CHARS)
    }
}

/// The church name, or the app name when the profile was never filled in.
/// Mirrors the Electron fallback that [`render_error`] already applies.
fn church_or_app(church: &str) -> &str {
    if church.trim().is_empty() {
        "SundayRec"
    } else {
        church
    }
}

/// The last path component of `path`, splitting on BOTH separators so a Windows
/// path is handled on macOS and vice versa. Returns the whole string when there
/// is no separator (the normal case: a bare file name).
fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
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
    fn render_test_carries_both_body_parts() {
        // It carried only plaintext until the relay landed; the endpoint
        // requires both, and `RelayMessage::fits` is where that is enforced.
        // Kept as a named test rather than folded into the loop below because
        // the change of shape is the interesting part.
        let r = render_test(MailLang::No);
        assert_eq!(r.subject, "✓ SundayRec — e-post fungerer");
        assert!(r.text.contains("testmelding"));
        assert!(r.html.contains("<p>"));
        assert!(r.html.contains("testmelding"));
    }

    // ── The relay's messages ─────────────────────────────────────────────────

    /// Every language, so a loop reads as a table rather than seven copies.
    const LANGS: [MailLang; 7] = [
        MailLang::No,
        MailLang::En,
        MailLang::De,
        MailLang::Sv,
        MailLang::Da,
        MailLang::Pl,
        MailLang::Fr,
    ];

    #[test]
    fn all_seven_languages_have_complete_relay_catalogs() {
        for lang in LANGS {
            let m = missed_strings(lang);
            assert!(m.subject_one.contains("{church}"), "{lang:?} missed one");
            assert!(m.subject_one.contains("{date}"), "{lang:?} missed one date");
            assert!(m.subject_many.contains("{count}"), "{lang:?} missed count");
            assert!(m.subject_many.contains("{church}"), "{lang:?} missed many");
            assert!(m.subject_many.contains("{date}"), "{lang:?} missed oldest");
            assert!(m.greeting.contains("{name}"), "{lang:?} missed greeting");
            assert!(m.intro_one.contains("{church}"), "{lang:?} missed intro");
            assert!(m.intro_many.contains("{count}"), "{lang:?} intro count");
            assert!(m.intro_many.contains("{church}"), "{lang:?} intro church");
            assert!(!m.list_label.is_empty() && !m.instruction.is_empty());

            let r = receipt_strings(lang);
            assert!(r.subject.contains("{church}"), "{lang:?} receipt church");
            assert!(r.subject.contains("{date}"), "{lang:?} receipt date");
            assert!(r.greeting.contains("{name}"), "{lang:?} receipt greeting");
            assert!(r.intro.contains("{church}"), "{lang:?} receipt intro");
            for label in [
                r.slot_label,
                r.started_label,
                r.duration_label,
                r.file_label,
            ] {
                assert!(!label.is_empty(), "{lang:?} receipt label");
            }
            assert!(!r.hour_short.is_empty() && !r.minute_short.is_empty());

            let c = confirm_strings(lang);
            assert!(c.greeting.contains("{name}"), "{lang:?} confirm greeting");
            assert!(c.intro.contains("{church}"), "{lang:?} confirm church");
            for line in [c.subject, c.what, c.button, c.link_hint, c.expiry, c.ignore] {
                assert!(!line.is_empty(), "{lang:?} confirm line");
            }

            let f = footer_strings(lang);
            assert!(!f.note.is_empty() && !f.link_label.is_empty(), "{lang:?}");
        }
    }

    #[test]
    fn the_confirm_mail_promises_the_ttl_the_endpoint_enforces() {
        // Seven hand-written sentences carrying one number the Worker owns.
        // If the token TTL ever moves, this fails in every language at once
        // rather than leaving seven mails quietly promising the old figure.
        let days = crate::relay::CONFIRM_LINK_TTL_DAYS.to_string();
        for lang in LANGS {
            assert!(
                confirm_strings(lang).expiry.contains(&days),
                "{lang:?} expiry line must name {days} days"
            );
        }
    }

    #[test]
    fn missed_says_one_thing_for_one_and_another_for_several() {
        let one = [MissedOccurrence {
            label: "Søndag formiddag",
            date: "31.05.2026 11:00",
        }];
        let r = render_missed(MailLang::No, "Oslo domkirke", "Ola", &one).expect("one");
        assert_eq!(
            r.subject,
            "⚠️ Planlagt opptak ble ikke gjort — Oslo domkirke — 31.05.2026 11:00"
        );
        assert!(r.text.starts_with("Hei Ola,\n"));
        assert!(r.text.contains("- Søndag formiddag — 31.05.2026 11:00"));
        assert!(r.text.trim_end().ends_with("Hilsen SundayRec"));

        let three = [
            MissedOccurrence {
                label: "Søndag formiddag",
                date: "17.05.2026 11:00",
            },
            MissedOccurrence {
                label: "Søndag formiddag",
                date: "24.05.2026 11:00",
            },
            MissedOccurrence {
                label: "Kveldsmesse",
                date: "31.05.2026 19:00",
            },
        ];
        let r = render_missed(MailLang::No, "Oslo domkirke", "Ola", &three).expect("three");
        // ONE mail, headlined by the oldest — not three mails, and not a
        // headline pointing at the most recent one.
        assert!(r
            .subject
            .starts_with("⚠️ 3 planlagte opptak ble ikke gjort"));
        assert!(r.subject.ends_with("eldste 17.05.2026 11:00"));
        assert!(r.text.contains("Kveldsmesse — 31.05.2026 19:00"));
        assert_eq!(r.html.matches("<li>").count(), 3);
    }

    #[test]
    fn an_empty_missed_list_is_not_a_message() {
        // The occurrence filter (`notify_seen`) can legitimately empty the list
        // — every slot in this sweep was already reported. `None` says so; a
        // mail saying "0 recordings were missed" would be worse than silence.
        assert!(render_missed(MailLang::No, "Kirka", "Ola", &[]).is_none());
    }

    #[test]
    fn a_receipt_carries_the_file_name_and_never_the_folder() {
        // The strongest single assertion in the receipt: hand it a full path,
        // including the space-containing file name that defeats the free-text
        // scrubber, and the folder does not survive the rendering.
        let r = render_receipt(
            MailLang::No,
            "Oslo domkirke",
            "Ola",
            "Søndag formiddag",
            "31.05.2026 11:00",
            4_980,
            "/Users/kari/Opptak/gudstjeneste 9. november.wav",
        );
        assert!(r.text.contains("Fil: gudstjeneste 9. november.wav"));
        assert!(r.html.contains("gudstjeneste 9. november.wav"));
        for body in [&r.subject, &r.text, &r.html] {
            assert!(!body.contains("/Users"), "the folder leaked into {body}");
            assert!(!body.contains("Opptak/"), "the folder leaked into {body}");
        }
        assert!(r.text.contains("Varighet: 1 t 23 min"));
        assert_eq!(
            r.subject,
            "✓ Opptaket er ferdig — Oslo domkirke — 31.05.2026 11:00"
        );
        // A Windows path is handled on any host, and a bare name is untouched.
        let win = render_receipt(
            MailLang::En,
            "St Mary",
            "Ann",
            "Sunday",
            "31/05/2026 11:00",
            60,
            r"C:\Users\kari\Opptak\service.wav",
        );
        assert!(win.text.contains("File: service.wav"));
        assert!(!win.text.contains("kari"));
        let bare = render_receipt(MailLang::En, "St Mary", "Ann", "S", "d", 60, "x.wav");
        assert!(bare.text.contains("File: x.wav"));
    }

    #[test]
    fn a_duration_reads_as_a_length_in_every_language() {
        assert_eq!(format_duration(MailLang::No, 4_980), "1 t 23 min");
        assert_eq!(format_duration(MailLang::No, 3_600), "1 t");
        assert_eq!(format_duration(MailLang::No, 1_380), "23 min");
        // Sub-minute rounds down rather than pretending to a precision a
        // receipt does not need.
        assert_eq!(format_duration(MailLang::No, 59), "0 min");
        assert_eq!(format_duration(MailLang::De, 4_980), "1 Std. 23 Min.");
        assert_eq!(format_duration(MailLang::Fr, 4_980), "1 h 23 min");
        for lang in LANGS {
            assert!(!format_duration(lang, 4_980).is_empty());
        }
    }

    #[test]
    fn the_confirmation_mail_shows_the_link_twice_and_offers_a_way_out() {
        let url = "https://notify.sundaysuite.app/v1/notify/confirm?s=018f&t=abc";
        let r = render_confirm(MailLang::No, "Oslo domkirke", "Ola", url);
        assert_eq!(r.subject, "Bekreft e-postvarsler fra SundayRec");
        // Bare URL in the plaintext…
        assert!(r.text.contains(url));
        // …href AND visible text in the HTML, for the client that eats buttons.
        // Escaped, because a raw `&` between query parameters is invalid inside
        // an attribute and some clients truncate the link at it.
        assert_eq!(r.html.matches(&esc(url)).count(), 3);
        assert!(r.html.contains("s=018f&amp;t=abc"));
        assert!(!r.html.contains("s=018f&t=abc"));
        assert!(r.html.contains("Bekreft e-postadressen"));
        // The plaintext says what the button says, then the bare URL — it does
        // not ask "button not working?" where there is no button.
        assert!(r.text.contains("Bekreft e-postadressen:\nhttps://"));
        assert!(!r.text.contains("Virker ikke knappen"));
        // Doing nothing is a complete answer, and the mail says so.
        assert!(r.text.contains("se bort fra denne e-posten"));
        assert!(r.text.contains("7 dager"));
    }

    #[test]
    fn the_footer_never_hides_itself_behind_a_signature_delimiter() {
        // `-- ` alone on a line is the RFC 3676 signature delimiter; a client
        // that honours it would collapse the unsubscribe link out of sight.
        for lang in LANGS {
            let f = render_unsubscribe_footer(lang, "https://notify.sundaysuite.app/u");
            for line in f.text.lines() {
                assert_ne!(line.trim_end(), "--", "{lang:?} footer hides itself");
            }
            assert!(f.text.starts_with("\n\n"), "{lang:?} footer separation");
        }
    }

    #[test]
    fn the_unsubscribe_footer_lands_on_everything_but_the_confirmation() {
        let unsub = "https://notify.sundaysuite.app/v1/notify/unsubscribe?s=018f&t=def";
        for lang in LANGS {
            let label = footer_strings(lang).link_label;
            for kind in RelayMessageKind::ALL.iter().copied() {
                let rendered = match kind {
                    RelayMessageKind::Confirm => {
                        render_confirm(lang, "Kirka", "Ola", "https://x/y")
                    }
                    RelayMessageKind::Failure => {
                        render_error(lang, "Kirka", "Ola", "31.05.2026 11:00", "boom")
                    }
                    RelayMessageKind::Missed => render_missed(
                        lang,
                        "Kirka",
                        "Ola",
                        &[MissedOccurrence {
                            label: "Søndag",
                            date: "31.05.2026 11:00",
                        }],
                    )
                    .expect("one occurrence"),
                    RelayMessageKind::Receipt => {
                        render_receipt(lang, "Kirka", "Ola", "Søndag", "d", 60, "x.wav")
                    }
                    RelayMessageKind::Test => render_test(lang),
                };
                let msg = RelayMessage::new(kind, lang, rendered, unsub);
                let wants = kind.wants_unsubscribe_footer();
                assert_eq!(
                    msg.text.contains(unsub),
                    wants,
                    "{lang:?}/{} plaintext footer",
                    kind.as_str()
                );
                assert_eq!(
                    msg.html.contains(&esc(unsub)),
                    wants,
                    "{lang:?}/{} html footer",
                    kind.as_str()
                );
                assert_eq!(
                    msg.text.contains(label),
                    wants,
                    "{lang:?}/{}",
                    kind.as_str()
                );
                assert!(msg.fits(), "{lang:?}/{} must fit", kind.as_str());
            }
        }
    }

    #[test]
    fn the_five_kinds_are_the_endpoints_five_words() {
        // The endpoint's KINDS list is closed; a word this side does not know
        // is a 400, and a 400 drops the alert permanently.
        let words: Vec<&str> = RelayMessageKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(words, ["confirm", "failure", "missed", "receipt", "test"]);
        for kind in RelayMessageKind::ALL.iter().copied() {
            let json = serde_json::to_string(&kind).expect("serialise");
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
        }
        // Only the confirmation goes without a footer.
        let bare: Vec<&str> = RelayMessageKind::ALL
            .iter()
            .filter(|k| !k.wants_unsubscribe_footer())
            .map(|k| k.as_str())
            .collect();
        assert_eq!(bare, ["confirm"]);
    }

    #[test]
    fn fits_demands_both_body_parts_and_all_three_caps() {
        let ok = RelayMessage {
            kind: RelayMessageKind::Test,
            subject: "s".into(),
            text: "t".into(),
            html: "<p>t</p>".into(),
        };
        assert!(ok.fits());
        for missing in [
            RelayMessage {
                subject: String::new(),
                ..ok.clone()
            },
            RelayMessage {
                text: String::new(),
                ..ok.clone()
            },
            RelayMessage {
                html: String::new(),
                ..ok.clone()
            },
        ] {
            assert!(!missing.fits(), "an empty part must not pass: {missing:?}");
        }
        for oversize in [
            RelayMessage {
                subject: "é".repeat(SUBJECT_MAX_CHARS + 1),
                ..ok.clone()
            },
            RelayMessage {
                text: "é".repeat(TEXT_MAX_CHARS + 1),
                ..ok.clone()
            },
            RelayMessage {
                html: "é".repeat(HTML_MAX_CHARS + 1),
                ..ok.clone()
            },
        ] {
            assert!(!oversize.fits(), "over the cap must not pass");
        }
        // Counted in CHARACTERS, not bytes: a Norwegian subject is not shorter
        // in Oslo than in London.
        assert!(RelayMessage {
            subject: "æ".repeat(SUBJECT_MAX_CHARS),
            ..ok.clone()
        }
        .fits());
    }

    #[test]
    fn an_absurd_input_is_caught_here_rather_than_by_a_400() {
        // A church that pasted an essay into the profile field. The subject
        // template carries it, the subject cap rejects it, and `fits` says so
        // BEFORE the row is queued — which matters because the endpoint's
        // answer would be a 400, and a 400 drops the alert without a retry.
        let long = "Menighet ".repeat(40);
        let msg = RelayMessage::new(
            RelayMessageKind::Failure,
            MailLang::No,
            render_error(MailLang::No, &long, "Ola", "31.05.2026 11:00", "boom"),
            "https://notify.sundaysuite.app/u",
        );
        assert!(msg.subject.chars().count() > SUBJECT_MAX_CHARS);
        assert!(!msg.fits());
    }

    // ── The repo-binding test ────────────────────────────────────────────────

    /// The endpoint's own rejection rule, MIRRORED — the relay's copy of the
    /// seam `telemetry::tests::scrubbed_free_text_is_accepted_by_the_endpoints_own_validator`
    /// guards for the telemetry payload.
    ///
    /// Copied verbatim (modulo Rust escaping) from `ABSOLUTE_PATH_RE` in
    /// `sunday-telemetry/src/validate.ts`, which the relay's validator runs over
    /// `subject`, `text` and `html` and answers `400 unscrubbed_path` to. A 400
    /// is a permanent drop in the outbox, so a rendering that trips this regex
    /// is an alert that vanishes silently — the exact failure mode the telemetry
    /// seam had before it was pinned.
    ///
    /// **The two must be changed together.** Two sides, one rule, both with a
    /// test: the Worker rejects, this decides not to send, and neither of them
    /// gets to hold a private opinion about what "clean" means.
    const WORKER_ABSOLUTE_PATH_RE: &str = r#"(^|[\s"'(<\[])(/(Users|home|var|tmp|private|Volumes)/|[A-Za-z]:[\\/]|\\\\[^\s\\]+\\|~[\\/])"#;

    #[test]
    fn relay_bodies_pass_the_endpoints_own_validator() {
        use crate::telemetry::{sanitize_free_text, MESSAGE_MAX_CHARS};
        let re = regex::Regex::new(WORKER_ABSOLUTE_PATH_RE).expect("the mirror must compile");

        // Hostile inputs, scrubbed exactly as the relay call site scrubs them.
        let raw_church = "Kirka i /Users/kari/Menighet";
        let raw_person = "Ola fra /home/ola";
        let raw_error = r#"kunne ikke åpne Some("/Users/kari/Opptak/gudstjeneste.wav")"#;
        let raw_slot = "Søndag /var/log/x";
        // The full path is deliberately NOT scrubbed: `render_receipt` reduces
        // it to a basename itself, which is the stronger guarantee.
        let raw_file = "/Users/kari/Opptak/gudstjeneste 9. november.wav";

        // The mirror is LIVE, not vacuous: it flags every raw input above.
        for hostile in [raw_church, raw_person, raw_error, raw_slot, raw_file] {
            assert!(
                re.is_match(hostile),
                "the mirror should reject the unscrubbed {hostile:?}"
            );
        }

        let church = sanitize_free_text(raw_church, Some("/Users/kari"), MESSAGE_MAX_CHARS);
        let person = sanitize_free_text(raw_person, None, MESSAGE_MAX_CHARS);
        let error = sanitize_free_text(raw_error, Some("/Users/kari"), MESSAGE_MAX_CHARS);
        let slot = sanitize_free_text(raw_slot, None, MESSAGE_MAX_CHARS);
        let date = "31.05.2026 11:00";
        let confirm_url = "https://notify.sundaysuite.app/v1/notify/confirm?s=018f2b&t=0f9a";
        let unsub_url = "https://notify.sundaysuite.app/v1/notify/unsubscribe?s=018f2b&t=7c1d";

        let mut checked = 0usize;
        for lang in LANGS {
            for kind in RelayMessageKind::ALL.iter().copied() {
                let rendered = match kind {
                    RelayMessageKind::Confirm => {
                        render_confirm(lang, &church, &person, confirm_url)
                    }
                    RelayMessageKind::Failure => render_error(lang, &church, &person, date, &error),
                    RelayMessageKind::Missed => render_missed(
                        lang,
                        &church,
                        &person,
                        &[
                            MissedOccurrence { label: &slot, date },
                            MissedOccurrence {
                                label: "Kveldsmesse",
                                date: "07.06.2026 19:00",
                            },
                        ],
                    )
                    .expect("two occurrences"),
                    RelayMessageKind::Receipt => {
                        render_receipt(lang, &church, &person, &slot, date, 4_980, raw_file)
                    }
                    RelayMessageKind::Test => render_test(lang),
                };
                let msg = RelayMessage::new(kind, lang, rendered, unsub_url);
                for (part, body) in [
                    ("subject", &msg.subject),
                    ("text", &msg.text),
                    ("html", &msg.html),
                ] {
                    assert!(
                        !re.is_match(body),
                        "the endpoint would answer 400 unscrubbed_path and the outbox \
                         would drop this alert without retrying.\n  lang: {lang:?}\n  \
                         kind: {}\n  part: {part}\n  body: {body}",
                        kind.as_str()
                    );
                    checked += 1;
                }
                // The size half of the same validator, on the same messages.
                assert!(msg.fits(), "{lang:?}/{} exceeds a cap", kind.as_str());
            }
        }
        assert_eq!(checked, 7 * 5 * 3, "every language × kind × body part");

        // And the footer alone, since it is appended after every check the
        // renderers do and carries a URL of its own.
        for lang in LANGS {
            let f = render_unsubscribe_footer(lang, unsub_url);
            assert!(!re.is_match(&f.text) && !re.is_match(&f.html), "{lang:?}");
        }
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
