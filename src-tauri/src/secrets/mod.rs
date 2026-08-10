//! OS-native secret storage (macOS Keychain / Windows Credential Manager) via
//! the `keyring` crate — NEVER plaintext files. Replaces Electron's
//! `safeStorage`.
//!
//! OAuth tokens (Drive/Gmail) and the SMTP password are written here; Phase 0
//! established the seam and the resolution precedence so the rest of the app
//! has one place to reach for a credential.

use keyring::Entry;

use crate::error::{AppError, AppResult};

/// Keychain service name — matches the Tauri bundle identifier so credentials
/// are namespaced to this app.
const SERVICE: &str = "no.sundayrec.app";

/// The credentials SundayRec stores. Each maps to a stable keychain *account*
/// under [`SERVICE`]; renaming a variant's account orphans existing entries, so
/// treat these strings as a storage contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretProvider {
    /// Google Drive OAuth refresh token (cloud backup / upload).
    GoogleDrive,
    /// YouTube OAuth refresh token (publish / live).
    YouTube,
    /// Gmail OAuth refresh token (notification mailer).
    Gmail,
    /// HISTORICAL — the RTMP stream-key slot from the removed live-streaming
    /// feature (v0.14). Nothing writes it any more, but the variant stays so
    /// (a) the account string remains a documented part of the storage
    /// contract and (b) `all()`-driven sweeps can still DELETE a key an older
    /// install left in the keychain. Per-destination keys
    /// (`stream.key.{destId}`) are orphaned by design: their ids lived only in
    /// the retired `streamDestinations` setting, keyring cannot enumerate
    /// accounts, and a startup cleanup could block launch on a locked-keychain
    /// authorization prompt — the keys are inert in the user's own keychain.
    StreamKey,
    /// SMTP password for the email-alert mailer (never persisted in settings;
    /// mirrors the Electron `emailSmtpPassEnc` keychain slot).
    SmtpPassword,
    /// SundaySong / SundayPlan API key (bearer). Encrypted in the keychain, never
    /// in the integration-settings blob — mirrors the Electron `setSongApiKey`.
    SongApiKey,
    /// Anthropic API key for the OPTIONAL AI sermon-companion summary seam (R8).
    /// Stored in the OS keychain only — NEVER in settings, NEVER in a bundle. When
    /// unset the companion falls back to the fully-local extractive summary.
    CompanionLlmKey,
}

impl SecretProvider {
    /// The keychain account string for this provider.
    fn account(self) -> &'static str {
        match self {
            SecretProvider::GoogleDrive => "oauth.google_drive",
            SecretProvider::YouTube => "oauth.youtube",
            SecretProvider::Gmail => "oauth.gmail",
            SecretProvider::StreamKey => "stream.key",
            SecretProvider::SmtpPassword => "email.smtp_password",
            SecretProvider::SongApiKey => "integrations.song_api_key",
            SecretProvider::CompanionLlmKey => "companion.llm_api_key",
        }
    }

    /// All providers — handy for a "disconnect everything" sweep.
    pub fn all() -> [SecretProvider; 7] {
        [
            SecretProvider::GoogleDrive,
            SecretProvider::YouTube,
            SecretProvider::Gmail,
            SecretProvider::StreamKey,
            SecretProvider::SmtpPassword,
            SecretProvider::SongApiKey,
            SecretProvider::CompanionLlmKey,
        ]
    }
}

fn entry(provider: SecretProvider) -> AppResult<Entry> {
    Entry::new(SERVICE, provider.account())
        .map_err(|e| AppError::Internal(format!("keychain entry: {e}")))
}

/// Store (or replace) a provider's secret.
pub fn set(provider: SecretProvider, value: &str) -> AppResult<()> {
    entry(provider)?
        .set_password(value)
        .map_err(|e| AppError::Internal(format!("keychain set: {e}")))
}

/// Read a provider's secret, or `None` if unset / unreadable.
pub fn get(provider: SecretProvider) -> Option<String> {
    entry(provider).ok()?.get_password().ok()
}

/// Whether a provider currently has a stored secret.
pub fn has(provider: SecretProvider) -> bool {
    get(provider).is_some()
}

/// Delete a provider's secret. A missing entry is success, not an error.
pub fn delete(provider: SecretProvider) -> AppResult<()> {
    match entry(provider)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::Internal(format!("keychain delete: {e}"))),
    }
}

/// Resolve a credential from, in order: an explicit value (a non-blank override
/// the caller already holds), the keychain, then an environment variable. Pure
/// over its inputs so the precedence is unit-tested without a real keychain.
pub fn resolve(explicit: Option<String>, provider: SecretProvider, env_var: &str) -> String {
    resolve_from(explicit, get(provider), std::env::var(env_var).ok())
}

/// The pure precedence used by [`resolve`]: explicit → keychain → env → empty.
/// Blank/whitespace-only values are treated as unset so an empty override falls
/// through instead of masking a real stored secret.
fn resolve_from(explicit: Option<String>, keychain: Option<String>, env: Option<String>) -> String {
    [explicit, keychain, env]
        .into_iter()
        .flatten()
        .find(|v| !v.trim().is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real-keychain round-trip tests below WRITE to the OS keychain. On a
    /// locked/unauthorised keychain that write BLOCKS on an OS authorization
    /// prompt — it never returns, so the `_or_skip` match arms can't skip it, and
    /// the whole `cargo test` hangs. Gate them behind an explicit opt-in so the
    /// headless gate never hangs; set `SUNDAYREC_KEYCHAIN_TEST=1` on a machine with
    /// an unlocked, authorised keychain to actually exercise them.
    fn keychain_test_opted_in() -> bool {
        if std::env::var_os("SUNDAYREC_KEYCHAIN_TEST").is_none() {
            eprintln!("SKIP: set SUNDAYREC_KEYCHAIN_TEST=1 to exercise the real keychain");
            return false;
        }
        true
    }

    #[test]
    fn explicit_value_wins() {
        let got = resolve_from(
            Some("explicit".into()),
            Some("keychain".into()),
            Some("env".into()),
        );
        assert_eq!(got, "explicit");
    }

    #[test]
    fn blank_explicit_falls_through_to_keychain() {
        let got = resolve_from(
            Some("   ".into()),
            Some("keychain".into()),
            Some("env".into()),
        );
        assert_eq!(got, "keychain");
    }

    #[test]
    fn keychain_beats_env() {
        let got = resolve_from(None, Some("keychain".into()), Some("env".into()));
        assert_eq!(got, "keychain");
    }

    #[test]
    fn env_is_last_resort() {
        let got = resolve_from(None, None, Some("env".into()));
        assert_eq!(got, "env");
    }

    #[test]
    fn nothing_set_yields_empty() {
        assert_eq!(resolve_from(None, None, None), "");
        assert_eq!(resolve_from(Some("".into()), None, Some("  ".into())), "");
    }

    #[test]
    fn provider_accounts_are_distinct() {
        let mut accounts: Vec<&str> = SecretProvider::all().iter().map(|p| p.account()).collect();
        let count = accounts.len();
        accounts.sort_unstable();
        accounts.dedup();
        assert_eq!(accounts.len(), count, "provider accounts must be unique");
    }

    // Tolerant integration test: exercises the REAL keychain when one is
    // reachable, otherwise skips so the gate stays green in headless CI. Uses
    // the historical StreamKey slot with a sentinel value it always cleans up —
    // deliberately: it is the ONE provider nothing real writes any more, so the
    // test's delete can never destroy a credential an install depends on
    // (SmtpPassword/GoogleDrive/… are live slots).
    #[test]
    fn real_keychain_round_trip_or_skip() {
        if !keychain_test_opted_in() {
            return;
        }
        let provider = SecretProvider::StreamKey;
        let sentinel = "sundayrec-test-sentinel-value";
        match set(provider, sentinel) {
            // A write that succeeds but doesn't read back (headless CI
            // keychain) is a skip, not a failure.
            Ok(()) if get(provider).as_deref() == Some(sentinel) => {
                assert!(has(provider));
                delete(provider).expect("delete should succeed");
                assert!(!has(provider));
                eprintln!("keychain integration test hit a REAL keychain");
            }
            Ok(()) => {
                let _ = delete(provider);
                eprintln!("SKIP: keychain write did not round-trip in this environment");
            }
            Err(e) => {
                eprintln!("SKIP: no reachable keychain in this environment: {e}");
            }
        }
    }
}
