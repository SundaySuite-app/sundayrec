//! Where the telemetry endpoint lives, and the key used to write to it.
//!
//! Same mechanism as the old cloud `GoogleOAuthConfig` used: a runtime env
//! var for dev and CI, falling back to an `option_env!` baked in at release
//! build time. `None` means "telemetry has no endpoint in this build", and the
//! sender is simply never constructed — reports keep queueing locally and
//! nothing reaches the network, which is the same state E3 shipped in.
//!
//! ## Why the write key is not a secret, and why it still exists
//!
//! It ships inside every SundayRec binary. Anyone with a copy of the app and a
//! hex editor has it, and no amount of obfuscation changes that — the same
//! argument Google makes about installed-app OAuth secrets, which is why the
//! precedent this file follows is the one that already handles a non-secret
//! credential.
//!
//! So it is not a defence and is not treated as one. It is a spam filter: it
//! stops the endpoint being an open write target for anything that finds the
//! URL, and nothing more. Every guarantee that matters — that a payload cannot
//! carry a path, a device name, a church name, or an unknown field — is enforced
//! server-side in `sunday-telemetry/src/schema.ts`, which validates every
//! request and trusts no caller. Extracting the key buys the ability to post
//! well-formed anonymous telemetry, which is a thing anyone can already do.
//!
//! What it must NOT be is the same key that reads data back out. It is not: the
//! summary endpoint is behind a separate `ADMIN_KEY` that never leaves the
//! owner's password manager, so a key extracted from a binary cannot read the
//! fleet.
//!
//! ## Why a missing key is not "send anyway"
//!
//! [`resolve`] requires BOTH a URL and a key. A build with a URL and no key
//! would post to the endpoint and collect a 401 for every payload — six
//! attempts each, all doomed. Requiring both means such a build simply has no
//! sender, which is the quiet, correct failure.

/// The resolved telemetry endpoint for this build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryEndpoint {
    /// The ingest URL, e.g. `https://telemetry.sundaysuite.app/v1/ingest`.
    pub ingest_url: String,
    /// The base URL, for the deletion route.
    pub base_url: String,
    /// The static write key, sent as `x-sundayrec-key`.
    pub write_key: String,
}

/// The env/build var naming the endpoint's BASE url (no trailing path).
pub const BASE_URL_VAR: &str = "SUNDAYREC_TELEMETRY_URL";
/// The env/build var carrying the write key.
pub const WRITE_KEY_VAR: &str = "SUNDAYREC_TELEMETRY_KEY";

/// The write key for this build — ONE resolution, used by both pipes.
///
/// The e-mail relay posts to the same Worker with the same key (its routes are
/// app-scoped and read it from `x-write-key`; see
/// `crate::notify::relay::config`), so this is a function rather than a constant
/// the relay could re-derive: `option_env!` takes a string LITERAL, so a second
/// resolver would have to re-type `"SUNDAYREC_TELEMETRY_KEY"` — and two literals
/// spelling one build variable is exactly the kind of agreement that holds until
/// somebody renames one of them.
pub fn resolve_write_key() -> Option<String> {
    std::env::var(WRITE_KEY_VAR)
        .ok()
        .or_else(|| option_env!("SUNDAYREC_TELEMETRY_KEY").map(str::to_string))
}

impl TelemetryEndpoint {
    /// Resolve from the runtime env, falling back to the values baked in at
    /// build time under the same names. `None` when either is missing or blank.
    pub fn resolve() -> Option<Self> {
        let base = std::env::var(BASE_URL_VAR)
            .ok()
            .or_else(|| option_env!("SUNDAYREC_TELEMETRY_URL").map(str::to_string));
        Self::normalize(base, resolve_write_key())
    }

    /// Pure construction, so the rules are testable without touching the
    /// process environment.
    ///
    /// Rejects a non-`http(s)` base outright rather than letting a typo become a
    /// runtime error on every send — and rejects a blank key rather than sending
    /// an empty header that the endpoint would answer with 401 six times.
    pub fn normalize(base: Option<String>, key: Option<String>) -> Option<Self> {
        let base = base.map(|s| s.trim().trim_end_matches('/').to_string())?;
        if base.is_empty() || !(base.starts_with("https://") || base.starts_with("http://")) {
            return None;
        }
        let write_key = key
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())?;
        Some(Self {
            ingest_url: format!("{base}/v1/ingest"),
            base_url: base,
            write_key,
        })
    }

    /// The deletion URL for one retired install id.
    pub fn delete_url(&self, install_id: &str) -> String {
        format!("{}/v1/install/{install_id}", self.base_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_halves_are_required() {
        assert_eq!(TelemetryEndpoint::normalize(None, None), None);
        assert_eq!(
            TelemetryEndpoint::normalize(Some("https://t.example".into()), None),
            None,
            "a URL without a key would post and collect a 401 six times per payload"
        );
        assert_eq!(TelemetryEndpoint::normalize(None, Some("k".into())), None);
        assert_eq!(
            TelemetryEndpoint::normalize(Some("https://t.example".into()), Some("  ".into())),
            None,
            "a blank key is a missing key, not an empty header"
        );
    }

    #[test]
    fn a_non_http_base_is_refused_at_construction() {
        // Better a build with no sender than one that fails on every send.
        assert_eq!(
            TelemetryEndpoint::normalize(
                Some("telemetry.sundaysuite.app".into()),
                Some("k".into())
            ),
            None
        );
        assert_eq!(
            TelemetryEndpoint::normalize(Some("ftp://t.example".into()), Some("k".into())),
            None
        );
    }

    #[test]
    fn urls_are_built_from_a_normalised_base() {
        let c = TelemetryEndpoint::normalize(
            Some("https://telemetry.sundaysuite.app/".into()),
            Some(" key123 ".into()),
        )
        .expect("valid");
        assert_eq!(c.base_url, "https://telemetry.sundaysuite.app");
        assert_eq!(
            c.ingest_url, "https://telemetry.sundaysuite.app/v1/ingest",
            "a trailing slash on the base must not produce a double slash"
        );
        assert_eq!(c.write_key, "key123");
        assert_eq!(
            c.delete_url("018f3a2b-7c4d-7e1f-9a2b-3c4d5e6f7a8b"),
            "https://telemetry.sundaysuite.app/v1/install/018f3a2b-7c4d-7e1f-9a2b-3c4d5e6f7a8b"
        );
    }

    #[test]
    fn a_localhost_base_is_allowed_for_wrangler_dev() {
        // The E2E path runs against `wrangler dev` over plain http.
        let c = TelemetryEndpoint::normalize(
            Some("http://127.0.0.1:8787".into()),
            Some("dev-key".into()),
        )
        .expect("valid");
        assert_eq!(c.ingest_url, "http://127.0.0.1:8787/v1/ingest");
    }
}
