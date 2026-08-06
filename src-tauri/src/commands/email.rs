//! Email-alert commands (PU-1 P2b) — the thin IPC layer over `crate::email`.
//!
//! `email_status` reports whether this build can send (the `email` cargo
//! feature) and whether a Gmail refresh token is stored, so the renderer can
//! render the panel WITHOUT having to provoke a failed send. `email_send_test`
//! dispatches a localized "email works" test message via the chosen transport.
//!
//! The send path (SMTP socket / Gmail POST) is behind the `email` feature, which
//! is now **in `default` and in both release feature lists** — so a shipped build
//! can actually send. Only a `--no-default-features` build makes
//! `email_send_test` return `feature_disabled`, and the panel then shows a calm
//! "not built into this build" hint. NETWORK-UNVERIFIED against a real provider.

use tauri::State;

use sundayrec_core::email::{EmailStatus, EmailTransportKind};
use sundayrec_core::webhook::{build_webhook_body, WebhookPayload};

use crate::db::Db;
use crate::error::AppResult;
use crate::settings;

/// Whether this build can send email + whether Gmail is already connected. Works
/// in every build: `feature_built` reflects the compile-time `email` feature and
/// `gmail_connected` reads the keychain for a stored Gmail refresh token.
#[tauri::command]
pub fn email_status() -> EmailStatus {
    EmailStatus {
        feature_built: cfg!(feature = "email"),
        gmail_connected: crate::secrets::has(crate::secrets::SecretProvider::Gmail),
    }
}

/// Send a localized "email works" test message to `recipient` via `transport`.
///
/// For [`EmailTransportKind::Smtp`] the `host`/`port`/`user`/`pass`/`from` fields
/// are required (the pass is used once and dropped); for
/// [`EmailTransportKind::Gmail`] they're ignored and the stored Gmail token is
/// used. `language` picks the localized subject/body (defaults to Norwegian).
///
/// NETWORK-UNVERIFIED behind `--features email`; returns `feature_disabled` in
/// the default build.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
#[cfg_attr(not(feature = "email"), allow(unused_variables))]
pub async fn email_send_test(
    transport: EmailTransportKind,
    recipient: String,
    language: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    pass: Option<String>,
    from: Option<String>,
) -> AppResult<()> {
    #[cfg(not(feature = "email"))]
    {
        Err(crate::error::AppError::Validation(
            "feature_disabled: email requires a build with `--features email`".into(),
        ))
    }

    #[cfg(feature = "email")]
    {
        use crate::cloud::config::GoogleOAuthConfig;
        use crate::email::Transport;
        use crate::error::AppError;

        let transport = match transport {
            EmailTransportKind::Gmail => Transport::Gmail {
                config: GoogleOAuthConfig::resolve().ok_or_else(|| {
                    AppError::Validation("no_config: Google OAuth not configured".into())
                })?,
            },
            EmailTransportKind::Smtp => Transport::Smtp {
                host: host
                    .filter(|h| !h.trim().is_empty())
                    .ok_or_else(|| AppError::Validation("no_config: smtp host".into()))?,
                port: port.unwrap_or(587),
                user: user.filter(|u| !u.trim().is_empty()),
                pass: pass.ok_or_else(|| AppError::Validation("no_config: smtp pass".into()))?,
                from: from
                    .filter(|f| !f.trim().is_empty())
                    .ok_or_else(|| AppError::Validation("no_config: smtp from".into()))?,
            },
        };
        crate::email::send_test(&transport, &recipient, language.as_deref()).await
    }
}

/// Send a test notification to `url` to verify a webhook before relying on it
/// during a recording failure. The body shaping (URL validation, Slack/Discord
/// detection, the structured-vs-chat payload) is the unit-tested
/// [`sundayrec_core::webhook`]; the church name comes from settings. Returns the
/// `no_url` error for an invalid URL (matching the Electron handler). The POST
/// itself is NETWORK-UNVERIFIED (reuses the always-present `reqwest`, no feature).
#[tauri::command]
pub async fn email_test_webhook(db: State<'_, Db>, url: String) -> AppResult<bool> {
    let s = settings::load(&db.pool).await.unwrap_or_default();
    let ts = chrono::Utc::now().to_rfc3339();
    let payload = WebhookPayload::test(&s.church_name, &ts);
    let Some(body) = build_webhook_body(&url, &payload) else {
        return Err(crate::error::AppError::Validation("no_url".into()));
    };
    // NETWORK-UNVERIFIED: a 10 s-bounded POST; any non-success status / transport
    // error means the webhook isn't reachable (returns Ok(false), not an error,
    // so the panel shows "didn't work" rather than a stack-trace).
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
    Ok(matches!(resp, Ok(r) if r.status().is_success()))
}

/// Wipe the stored SMTP password from the OS keychain (the user disabled email
/// notifications or switched to Gmail). Mirrors the Electron `clear-smtp-password`
/// handler. A missing entry is success, not an error.
#[tauri::command]
pub fn email_clear_smtp_password() -> AppResult<bool> {
    crate::secrets::delete(crate::secrets::SecretProvider::SmtpPassword)?;
    Ok(true)
}

// ── The keychain WRITE path ──────────────────────────────────────────────────
//
// `SecretProvider::SmtpPassword` had a delete command and a (then unused) read,
// but nothing could ever PUT a password there: `secrets::set` was never called
// with it from any IPC entry point. So the SMTP password only ever existed for
// the duration of one "send test" request, and an unattended failure alert had
// no credential to send with. These two commands close that hole.
//
// Both are FEATURELESS on purpose — `crate::secrets` has no feature gate, and a
// `--no-default-features` build must still be able to store/inspect the
// credential (the user may configure the app before updating to a build that
// can send). Neither ever returns or logs the password itself.

/// What [`email_set_smtp_password`] should do with the value it was handed.
/// Pure so the "blank means clear" rule is unit-tested without a real keychain.
#[derive(Debug, PartialEq, Eq)]
enum PasswordAction {
    /// Store this (already trimmed of nothing — SMTP passwords may legitimately
    /// contain leading/trailing spaces, so only the *emptiness* test trims).
    Store(String),
    /// Remove any stored password.
    Clear,
}

/// `None`, `""` or whitespace-only → [`PasswordAction::Clear`]; anything else is
/// stored VERBATIM (Gmail app-passwords are commonly pasted with spaces, and
/// some servers accept passwords whose edges are meaningful — we must not
/// silently mangle the secret, only decide whether one was supplied at all).
fn password_action(password: Option<String>) -> PasswordAction {
    match password {
        Some(p) if !p.trim().is_empty() => PasswordAction::Store(p),
        _ => PasswordAction::Clear,
    }
}

/// Store the SMTP password in the OS keychain so unattended error alerts have a
/// credential to send with. Passing `null`/`""` clears it (same effect as
/// [`email_clear_smtp_password`]), which keeps the renderer's «Fjern» button and
/// an emptied field consistent. Returns `true` when a password is now stored,
/// `false` when the slot was cleared — NEVER the password itself.
#[tauri::command]
pub fn email_set_smtp_password(password: Option<String>) -> AppResult<bool> {
    match password_action(password) {
        PasswordAction::Store(pw) => {
            crate::secrets::set(crate::secrets::SecretProvider::SmtpPassword, &pw)?;
            Ok(true)
        }
        PasswordAction::Clear => {
            crate::secrets::delete(crate::secrets::SecretProvider::SmtpPassword)?;
            Ok(false)
        }
    }
}

/// Whether an SMTP password is currently stored in the keychain — drives the
/// renderer's "••••••••  (lagret)" state without ever reading the secret into
/// the webview.
#[tauri::command]
pub fn email_has_smtp_password() -> bool {
    crate::secrets::has(crate::secrets::SecretProvider::SmtpPassword)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The keychain itself is only exercised behind `SUNDAYREC_KEYCHAIN_TEST=1`
    // (see `crate::secrets::tests` — an unauthorised keychain BLOCKS on an OS
    // prompt and would hang the headless gate). What is testable here without a
    // keychain is the whole decision this command layer makes: when a write
    // happens at all, and that the secret is passed through untouched.

    #[test]
    fn a_real_password_is_stored_verbatim() {
        assert_eq!(
            password_action(Some("hunter2".into())),
            PasswordAction::Store("hunter2".into())
        );
    }

    #[test]
    fn app_password_spaces_are_preserved_not_trimmed() {
        // Gmail app-passwords are shown as "abcd efgh ijkl mnop"; mangling the
        // inner or edge spacing would break the login with no visible cause.
        assert_eq!(
            password_action(Some("abcd efgh ijkl mnop".into())),
            PasswordAction::Store("abcd efgh ijkl mnop".into())
        );
        assert_eq!(
            password_action(Some(" pad ".into())),
            PasswordAction::Store(" pad ".into())
        );
    }

    #[test]
    fn absent_or_blank_clears_instead_of_storing_an_empty_secret() {
        assert_eq!(password_action(None), PasswordAction::Clear);
        assert_eq!(password_action(Some(String::new())), PasswordAction::Clear);
        assert_eq!(
            password_action(Some("   \t\n".into())),
            PasswordAction::Clear
        );
    }
}
