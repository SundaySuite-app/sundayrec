//! Where the e-mail relay lives, and the key used to write to it.
//!
//! The mechanism is [`crate::telemetry::config`]'s, line for line: a runtime env
//! var for dev and CI, falling back to an `option_env!` baked in at release
//! build time. `None` means "this build has no relay endpoint", and the sender
//! is never constructed — nothing is queued, nothing is sent, and the settings
//! panel says so instead of offering a button that cannot work.
//!
//! ## Its own URL, the same key
//!
//! Two variables, and the asymmetry is deliberate.
//!
//! The URL is separate ([`BASE_URL_VAR`]) because the relay answers on
//! `notify.sundaysuite.app` while telemetry answers on `telemetry.`. Not for
//! routing — the Worker branches on the PATH, not the host name — but because a
//! confirmation link in a volunteer's inbox must not read
//! `https://telemetry.sundaysuite.app/…`. Somebody who has just been asked to
//! confirm an e-mail address and is shown a link with "telemetry" in it is being
//! told, by the only signal they can check, that this is a tracking mail. The
//! same reasoning that gave the updater its own host name in `update.rs`.
//!
//! The KEY is shared, via [`crate::telemetry::config::resolve_write_key`],
//! because it is the same key: one Worker, one app entry, one
//! `TELEMETRY_WRITE_KEY`. The relay's routes are app-scoped
//! (`/v1/apps/sundayrec/notify/…`) and read it from
//! [`WRITE_KEY_HEADER`] rather than telemetry's frozen `x-sundayrec-key`, but
//! `appWriteKeyEquals` compares it against the same stored value. Importing the
//! resolution instead of re-typing the variable name is the whole point: two
//! literals naming one build variable agree until somebody renames one.
//!
//! Everything the key's own module says about it still holds — it ships inside
//! every binary, it is a spam filter and not a defence, and the ADMIN key that
//! reads data back out is a different key that never leaves the owner's password
//! manager.
//!
//! ## Why both halves are required
//!
//! [`resolve`] wants a URL and a key. A build with a URL and no key would POST
//! and collect a 401 for every request — and a 4xx is a PERMANENT drop, so those
//! would not even queue up for a fixed build: the alerts would be gone. Better a
//! build with no relay at all, which is the quiet, correct failure.

use super::sender::RELAY_APP_SLUG;

/// The resolved relay endpoint for this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayEndpoint {
    /// The base URL, e.g. `https://notify.sundaysuite.app` — no trailing slash.
    pub base_url: String,
    /// The static write key, sent as [`WRITE_KEY_HEADER`].
    pub write_key: String,
}

/// The env/build var naming the relay's BASE url (no trailing path).
pub const BASE_URL_VAR: &str = "SUNDAYREC_NOTIFY_URL";

/// The env/build var carrying the write key — telemetry's, re-exported rather
/// than re-spelled. See the module docs.
pub use crate::telemetry::config::WRITE_KEY_VAR;

/// The header the app-scoped Worker routes read the write key from.
///
/// NOT telemetry's `x-sundayrec-key`, which is frozen for the two pre-existing
/// routes that name one app in a Worker now serving several. Every route added
/// since — the relay's included — takes `x-write-key` and gets the app from the
/// path (`sunday-telemetry/src/auth.ts`).
pub const WRITE_KEY_HEADER: &str = "x-write-key";

impl RelayEndpoint {
    /// Resolve from the runtime env, falling back to the values baked in at
    /// build time. `None` when either half is missing or blank.
    pub fn resolve() -> Option<Self> {
        let base = std::env::var(BASE_URL_VAR)
            .ok()
            .or_else(|| option_env!("SUNDAYREC_NOTIFY_URL").map(str::to_string));
        Self::normalize(base, crate::telemetry::config::resolve_write_key())
    }

    /// Pure construction, so the rules are testable without touching the
    /// process environment.
    ///
    /// Rejects a non-`http(s)` base outright rather than letting a typo become a
    /// runtime error on every send, and rejects a blank key rather than sending
    /// an empty header the endpoint answers with a 401 — which the outbox reads
    /// as permanent and drops.
    pub fn normalize(base: Option<String>, key: Option<String>) -> Option<Self> {
        let base = base.map(|s| s.trim().trim_end_matches('/').to_string())?;
        if base.is_empty() || !(base.starts_with("https://") || base.starts_with("http://")) {
            return None;
        }
        let write_key = key
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        Some(Self {
            base_url: base,
            write_key,
        })
    }

    /// `POST` — enrol an address and ask the endpoint to send the confirmation.
    pub fn subscribe_url(&self) -> String {
        format!(
            "{}/v1/apps/{RELAY_APP_SLUG}/notify/subscribe",
            self.base_url
        )
    }

    /// `POST` — deliver one already-rendered notification.
    pub fn send_url(&self) -> String {
        format!("{}/v1/apps/{RELAY_APP_SLUG}/notify/send", self.base_url)
    }

    /// `GET` (is it confirmed yet?) and `DELETE` (forget the address). One
    /// function because they are one resource, and a second builder would be a
    /// second place for the path to drift.
    pub fn subscription_url(&self, sub_id: &str) -> String {
        format!(
            "{}/v1/apps/{RELAY_APP_SLUG}/notify/subscription/{sub_id}",
            self.base_url
        )
    }

    /// The link inside the confirmation mail.
    ///
    /// App-LESS on purpose (`/v1/notify/…`, not `/v1/apps/sundayrec/notify/…`):
    /// this URL is read by a person in a mail client, and the sub id is enough
    /// for the endpoint to find the row. A shorter link is also a link somebody
    /// can look at before pressing it, which is the whole point of printing it
    /// in full beside the button.
    pub fn confirm_url(&self, sub_id: &str, token: &str) -> String {
        format!("{}/v1/notify/confirm/{sub_id}/{token}", self.base_url)
    }

    /// The link in every mail's footer (and the endpoint's `List-Unsubscribe`
    /// header — RFC 8058, one POST for both ways out).
    pub fn unsubscribe_url(&self, sub_id: &str, token: &str) -> String {
        format!("{}/v1/notify/unsubscribe/{sub_id}/{token}", self.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_halves_are_required() {
        assert_eq!(RelayEndpoint::normalize(None, None), None);
        assert_eq!(
            RelayEndpoint::normalize(Some("https://notify.example".into()), None),
            None,
            "a URL without a key would POST and collect a 401 — which is a PERMANENT drop, \
             so the alert would be gone rather than waiting for a fixed build"
        );
        assert_eq!(RelayEndpoint::normalize(None, Some("k".into())), None);
        assert_eq!(
            RelayEndpoint::normalize(Some("https://notify.example".into()), Some("  ".into())),
            None,
            "a blank key is a missing key, not an empty header"
        );
    }

    #[test]
    fn a_non_http_base_is_refused_at_construction() {
        assert_eq!(
            RelayEndpoint::normalize(Some("notify.sundaysuite.app".into()), Some("k".into())),
            None
        );
        assert_eq!(
            RelayEndpoint::normalize(Some("ftp://notify.example".into()), Some("k".into())),
            None
        );
    }

    #[test]
    fn urls_are_built_from_a_normalised_base() {
        let c = RelayEndpoint::normalize(
            Some("https://notify.sundaysuite.app/".into()),
            Some(" key123 ".into()),
        )
        .expect("valid");
        assert_eq!(c.base_url, "https://notify.sundaysuite.app");
        assert_eq!(
            c.subscribe_url(),
            "https://notify.sundaysuite.app/v1/apps/sundayrec/notify/subscribe",
            "a trailing slash on the base must not produce a double slash"
        );
        assert_eq!(
            c.send_url(),
            "https://notify.sundaysuite.app/v1/apps/sundayrec/notify/send"
        );
        assert_eq!(
            c.subscription_url("018f3a2b-7c4d-7e1f-9a2b-3c4d5e6f7a8b"),
            "https://notify.sundaysuite.app/v1/apps/sundayrec/notify/subscription/\
             018f3a2b-7c4d-7e1f-9a2b-3c4d5e6f7a8b"
        );
        assert_eq!(c.write_key, "key123");
        // The two links a PERSON sees carry no app segment — see `confirm_url`.
        assert_eq!(
            c.confirm_url("sub-1", "tok-1"),
            "https://notify.sundaysuite.app/v1/notify/confirm/sub-1/tok-1"
        );
        assert_eq!(
            c.unsubscribe_url("sub-1", "tok-2"),
            "https://notify.sundaysuite.app/v1/notify/unsubscribe/sub-1/tok-2"
        );
    }

    #[test]
    fn a_localhost_base_is_allowed_for_wrangler_dev() {
        let c =
            RelayEndpoint::normalize(Some("http://127.0.0.1:8787".into()), Some("dev-key".into()))
                .expect("valid");
        assert_eq!(
            c.send_url(),
            "http://127.0.0.1:8787/v1/apps/sundayrec/notify/send"
        );
    }

    #[test]
    fn the_write_key_variable_is_telemetrys_and_not_a_second_one() {
        // The re-export, pinned. A relay build and a telemetry build read ONE
        // secret out of ONE Actions variable; if this ever became two names, a
        // release would ship with half the pipes working.
        assert_eq!(WRITE_KEY_VAR, crate::telemetry::config::WRITE_KEY_VAR);
        assert_eq!(WRITE_KEY_VAR, "SUNDAYREC_TELEMETRY_KEY");
        assert_ne!(
            BASE_URL_VAR,
            crate::telemetry::config::BASE_URL_VAR,
            "the URLs are two host names on purpose — see the module docs"
        );
    }
}
