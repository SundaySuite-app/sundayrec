//! Auto-update status model — pure, GUI-free (R7 P2a).
//!
//! Ported from the Electron `src/main/updater.ts` (the behavioural spec). That
//! module wired `electron-updater`'s event stream
//! (`checking-for-update`/`update-available`/`download-progress`/
//! `update-downloaded`/`error`) to renderer IPC sends, gated its `check()` in
//! development (`process.env.NODE_ENV === 'development' → return`), and followed
//! the `autoUpdate` setting for `autoDownload`/`autoInstallOnAppQuit`.
//!
//! Tauri 2 replaces `electron-updater` with `tauri-plugin-updater` (a pull API:
//! `check()` → optional `Update` → `download_and_install()` with a progress
//! callback). The *plumbing* (network fetch, signature verify, install, relaunch)
//! is the impure half and lives in the `src-tauri` `update` seam behind the
//! default-off `updater` feature. THIS module is the pure half: the localized
//! status enum the renderer renders, the dev-mode check guard, the
//! progress-percentage math, and the semver "is this actually newer" decision —
//! all deterministic and unit-tested here.
//!
//! The status enum tags are camelCase so they line up 1:1 with the existing
//! `update.*` i18n catalog keys ported from Electron (`checking`, `available`,
//! `downloading`, `readyInstall`, `upToDate`, `error`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::settings::UpdateChannel;

/// The host the update feeds live under when the build does not override it.
///
/// Deliberately NOT `telemetry.sundaysuite.app`, even though the same Worker
/// answers both names. An update check runs whether or not the operator
/// consented to telemetry; serving it from a host called "telemetry" would make
/// a consent-declining install look like it phones the telemetry endpoint
/// anyway — true in hostname, false in meaning, and precisely the wrong
/// conclusion for anyone reading PRIVACY.md or their own firewall log. Two
/// names keep the two promises separately legible.
///
/// It also replaces the old GitHub feed
/// (`releases/latest/download/latest.json`), which always served whatever the
/// newest release happened to be: there was no way to STOP serving a bad
/// version once published. The Worker serves only explicitly promoted
/// manifests, which is what makes the kill-switch and the two rings possible.
pub const DEFAULT_UPDATE_BASE: &str = "https://updates.sundaysuite.app";

/// The feed URL for one channel: `{base}/v1/update/{stable|beta}`.
///
/// The path carries none of the plugin's `{{current_version}}`/`{{target}}`/
/// `{{arch}}` placeholders on purpose — the manifest the Worker returns holds
/// the full platforms map and the plugin picks its own entry out of it. A
/// trailing slash (or stray whitespace) on `base` is absorbed so a
/// `SUNDAYREC_UPDATE_BASE` override cannot produce a `//v1` that 404s.
pub fn channel_feed_url(base: &str, channel: UpdateChannel) -> String {
    format!(
        "{}/v1/update/{}",
        base.trim().trim_end_matches('/'),
        channel.as_tag()
    )
}

/// The current state of an update check/download, as the renderer renders it.
///
/// Mirrors the Electron `update-*` IPC events one-to-one. `Idle` is the
/// pre-check resting state (the renderer shows the "click to check" hint);
/// every other variant maps to an `update.<key>` i18n string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/UpdateStatus.ts")]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum UpdateStatus {
    /// No check has run yet (or the last one was cleared). Renderer shows the
    /// "click «Se etter oppdateringer»" hint.
    Idle,
    /// A check is in flight (`checking-for-update`).
    Checking,
    /// No newer version exists (`update-not-available`).
    UpToDate,
    /// A newer version exists but isn't downloaded yet (`update-available`).
    /// `version` is the target semver.
    Available { version: String },
    /// The new version is downloading (`download-progress`). `percent` is
    /// clamped 0..=100; `version` is the target.
    Downloading { version: String, percent: u8 },
    /// The new version is downloaded and will install on relaunch
    /// (`update-downloaded`). `version` is the target.
    ReadyToInstall { version: String },
    /// The check/download failed (`error`). `message` is the human-readable
    /// reason (already classified by the seam).
    Error { message: String },
}

impl UpdateStatus {
    /// The `update.<key>` i18n key this status renders under, so the renderer
    /// (and tests) can map a status to its localized string without a `match`.
    /// `Idle` → the "check hint"; the rest mirror the Electron event names.
    pub fn i18n_key(&self) -> &'static str {
        match self {
            UpdateStatus::Idle => "update.checkHint",
            UpdateStatus::Checking => "update.checking",
            UpdateStatus::UpToDate => "update.upToDate",
            UpdateStatus::Available { .. } => "update.available",
            UpdateStatus::Downloading { .. } => "update.downloading",
            UpdateStatus::ReadyToInstall { .. } => "update.readyInstall",
            UpdateStatus::Error { .. } => "update.error",
        }
    }

    /// Whether this status represents a finished, installable download — the
    /// only state in which the "restart & install" action is meaningful.
    pub fn is_ready_to_install(&self) -> bool {
        matches!(self, UpdateStatus::ReadyToInstall { .. })
    }
}

/// Compute a clamped download percentage from bytes — the
/// `download-progress` `prog.percent` Electron exposed, recomputed here so the
/// seam can feed raw `(downloaded, total)` from the plugin's chunk callback.
/// A zero/unknown total yields 0 (the plugin reports `ContentLength` only when
/// the server sends it); a download past 100% is clamped.
pub fn download_percent(downloaded: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    let pct = (downloaded.saturating_mul(100)) / total;
    pct.min(100) as u8
}

/// Whether an update check should actually hit the network.
///
/// Direct port of the Electron `check()` guard
/// (`if (process.env.NODE_ENV === 'development') return`): in a dev build there
/// is no signed release to update to, so a check would only error. `is_dev` is
/// supplied by the seam (`cfg!(debug_assertions)` / `tauri::is_dev()`); this
/// keeps the policy testable.
pub fn should_check(is_dev: bool) -> bool {
    !is_dev
}

/// Whether `candidate` is strictly newer than `current` under semver ordering.
///
/// The updater plugin already gates on this server-side, but we re-check so a
/// misconfigured feed (e.g. a re-published same-version `latest.json`) never
/// surfaces a phantom "update available" to the user. Non-semver strings fall
/// back to a byte comparison so a tag like `"2026.05.31"` still orders sanely.
///
/// This re-check OVERRIDES the plugin: `src-tauri/src/update/mod.rs` turns a
/// `false` here into `UpToDate` no matter what the feed offered. So a version
/// pair this function gets wrong is a version pair no client can ever cross —
/// which is why the prerelease half below is spelled out rather than
/// approximated. `docs/ROLLBACK.md` leans on the same property in the other
/// direction ("a client only ever moves to a HIGHER version"), so this must
/// stay strictly monotone: never `true` for an equal or lower candidate.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_semver(candidate), parse_semver(current)) {
        (Some(c), Some(cur)) => c > cur,
        // If either side isn't clean semver, only treat as newer when the
        // strings genuinely differ (avoids a same-string false positive).
        _ => candidate != current,
    }
}

/// One dot-separated field of a prerelease tag (the `beta` and the `1` of
/// `-beta.1`).
///
/// The variant ORDER is load-bearing, not cosmetic: the derived `Ord` orders by
/// variant first, and semver §11 requires that a purely numeric identifier
/// always ranks BELOW an alphanumeric one (`1.0.0-1 < 1.0.0-alpha`). Reordering
/// these two lines silently changes update ordering for the whole fleet.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PreIdent {
    /// All-ASCII-digit field, compared as a NUMBER. This is the difference
    /// between `beta.10` superseding `beta.9` and the beta ring quietly
    /// freezing at the ninth beta — as strings, `"10" < "9"`.
    Numeric(u64),
    /// Anything else, compared lexically in ASCII order (`alpha < beta < rc`).
    Alpha(String),
}

/// A parsed semver version, ordered per semver.org §11.
#[derive(Debug, PartialEq, Eq)]
struct SemVer {
    major: u64,
    minor: u64,
    patch: u64,
    /// Empty for a plain release; one entry per dot-separated prerelease field.
    /// Build metadata (`+…`) is deliberately absent — §10 says it is ignored
    /// for precedence, so `1.0.0+a` and `1.0.0+b` must compare Equal.
    pre: Vec<PreIdent>,
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        use core::cmp::Ordering;

        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A version WITH a prerelease ranks below the same core
                // WITHOUT one: `0.11.0-beta.1 < 0.11.0`. This is the case the
                // whole two-ring flow depends on — a tester sitting on
                // `-beta.1` has to be able to reach the stable `0.11.0` that
                // the beta was testing FOR. Comparing only the core made these
                // equal, so that machine stayed on a beta forever, silently.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                // Both prereleases: field-by-field, left to right. `Vec`'s own
                // `Ord` is exactly the rule §11 states — compare elementwise,
                // and if every field of the shorter side matches, the side with
                // MORE fields wins (`alpha < alpha.1`).
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Parse `[v]MAJOR.MINOR.PATCH[-prerelease][+build]` into a comparable value.
/// Returns `None` if it isn't clean semver, which sends [`is_newer`] to its
/// string-difference fallback (see the `"2026.05.31"` test).
fn parse_semver(v: &str) -> Option<SemVer> {
    let s = v.trim_start_matches('v');

    // Build metadata goes first and unconditionally: it is ignored for
    // precedence (§10), and stripping it here keeps a `+` out of the
    // prerelease fields below.
    let s = s.split('+').next().unwrap_or(s);

    // Split core from prerelease at the FIRST `-` only. A prerelease may itself
    // contain hyphens (`1.2.3-beta-1` is one identifier, "beta-1"), so a
    // split-on-every-hyphen would mangle it.
    let (core, pre_str) = match s.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (s, None),
    };

    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None; // too many components — not a clean semver core
    }

    let pre = match pre_str {
        None => Vec::new(),
        Some(p) => {
            let mut fields = Vec::new();
            for ident in p.split('.') {
                // An empty field (`1.2.3-` or `1.2.3-beta..1`) is not valid
                // semver. Rejecting the whole parse is safer than guessing:
                // the caller then falls back to "newer only if the strings
                // differ", which cannot invent an ordering out of nothing.
                if ident.is_empty() {
                    return None;
                }
                if ident.bytes().all(|b| b.is_ascii_digit()) {
                    // An all-digit field too large for u64 is not something a
                    // release tag produces; treating it as alphanumeric keeps
                    // the parse total instead of failing the whole comparison.
                    match ident.parse::<u64>() {
                        Ok(n) => fields.push(PreIdent::Numeric(n)),
                        Err(_) => fields.push(PreIdent::Alpha(ident.to_string())),
                    }
                } else {
                    fields.push(PreIdent::Alpha(ident.to_string()));
                }
            }
            fields
        }
    };

    Some(SemVer {
        major,
        minor,
        patch,
        pre,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_maps_to_the_check_hint() {
        assert_eq!(UpdateStatus::Idle.i18n_key(), "update.checkHint");
    }

    #[test]
    fn each_phase_maps_to_its_electron_event_key() {
        assert_eq!(UpdateStatus::Checking.i18n_key(), "update.checking");
        assert_eq!(UpdateStatus::UpToDate.i18n_key(), "update.upToDate");
        assert_eq!(
            UpdateStatus::Available {
                version: "1.2.3".into()
            }
            .i18n_key(),
            "update.available"
        );
        assert_eq!(
            UpdateStatus::Downloading {
                version: "1.2.3".into(),
                percent: 40
            }
            .i18n_key(),
            "update.downloading"
        );
        assert_eq!(
            UpdateStatus::ReadyToInstall {
                version: "1.2.3".into()
            }
            .i18n_key(),
            "update.readyInstall"
        );
        assert_eq!(
            UpdateStatus::Error {
                message: "boom".into()
            }
            .i18n_key(),
            "update.error"
        );
    }

    #[test]
    fn only_ready_to_install_is_installable() {
        assert!(UpdateStatus::ReadyToInstall {
            version: "1.0.0".into()
        }
        .is_ready_to_install());
        assert!(!UpdateStatus::Downloading {
            version: "1.0.0".into(),
            percent: 99
        }
        .is_ready_to_install());
        assert!(!UpdateStatus::Idle.is_ready_to_install());
    }

    #[test]
    fn download_percent_is_clamped_and_zero_safe() {
        assert_eq!(download_percent(0, 0), 0); // unknown total
        assert_eq!(download_percent(50, 200), 25);
        assert_eq!(download_percent(200, 200), 100);
        assert_eq!(download_percent(300, 200), 100); // past 100% clamps
        assert_eq!(download_percent(1, 0), 0); // total 0 never divides
    }

    #[test]
    fn dev_builds_never_check() {
        assert!(!should_check(true));
        assert!(should_check(false));
    }

    #[test]
    fn is_newer_compares_semver_components() {
        assert!(is_newer("1.2.4", "1.2.3"));
        assert!(is_newer("1.3.0", "1.2.9"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.2.3", "1.2.3"));
        assert!(!is_newer("1.2.2", "1.2.3"));
    }

    #[test]
    fn is_newer_strips_v_prefix_and_prerelease() {
        assert!(is_newer("v1.2.4", "1.2.3"));
        // A prerelease of the same core is NOT newer than the release — semver
        // §11: `1.2.3-beta.1 < 1.2.3`. (This assertion predates prerelease
        // ordering, where it held for the accidental reason that both sides
        // collapsed to the same core. It holds for the RIGHT reason now, and a
        // failure here would mean stable installs being offered betas.)
        assert!(!is_newer("1.2.3-beta.1", "1.2.3"));
    }

    /// THE two-ring case: a machine promoted onto `0.11.0-beta.1` must be able
    /// to reach the stable `0.11.0` its Sunday was testing for. Comparing only
    /// `(major, minor, patch)` made these equal, so the beta ring was a
    /// one-way door — the tester stayed on a prerelease indefinitely, and the
    /// app told them "Du er oppdatert" while doing it.
    #[test]
    fn a_beta_tester_reaches_the_stable_release_of_the_same_version() {
        assert!(is_newer("0.11.0", "0.11.0-beta.1"));
        assert!(is_newer("0.11.0", "0.11.0-beta.9"));
        assert!(is_newer("1.2.3", "1.2.3-rc.1"));
    }

    /// The beta ring has to be able to ITERATE: promoting `beta.2` over
    /// `beta.1` must actually reach the testers already on `beta.1`.
    #[test]
    fn a_newer_beta_supersedes_an_older_beta() {
        assert!(is_newer("0.11.0-beta.2", "0.11.0-beta.1"));
        assert!(!is_newer("0.11.0-beta.1", "0.11.0-beta.2"));
    }

    /// Numeric prerelease fields compare as NUMBERS, not strings. As strings
    /// `"10" < "9"`, so the ring would break at the tenth beta of a version —
    /// long after anyone would think to suspect the comparator.
    #[test]
    fn numeric_prerelease_fields_compare_numerically_not_lexically() {
        assert!(is_newer("0.11.0-beta.10", "0.11.0-beta.9"));
        assert!(!is_newer("0.11.0-beta.9", "0.11.0-beta.10"));
        assert!(is_newer("1.0.0-beta.100", "1.0.0-beta.99"));
    }

    /// The rest of semver §11's prerelease rules, in the order the spec states
    /// them: alphanumeric fields compare in ASCII order, more fields beat
    /// fewer when everything before them matches, and a numeric field ranks
    /// below an alphanumeric one.
    #[test]
    fn prerelease_identifiers_follow_semver_precedence() {
        assert!(is_newer("1.0.0-rc.1", "1.0.0-beta.1"));
        assert!(is_newer("1.0.0-beta.1", "1.0.0-alpha.1"));
        assert!(!is_newer("1.0.0-beta.1", "1.0.0-rc.1"));

        // More fields win when the shared prefix is equal.
        assert!(is_newer("1.0.0-alpha.1", "1.0.0-alpha"));
        assert!(!is_newer("1.0.0-alpha", "1.0.0-alpha.1"));

        // Numeric always ranks below alphanumeric (the spec's own example).
        assert!(is_newer("1.0.0-alpha", "1.0.0-1"));
        assert!(!is_newer("1.0.0-1", "1.0.0-alpha"));

        // The core still outranks everything: a lower core with a higher-
        // sorting prerelease is NOT newer.
        assert!(is_newer("0.11.0-beta.1", "0.10.0"));
        assert!(!is_newer("0.10.0-rc.9", "0.11.0-alpha.1"));
    }

    /// Equal is never newer — including when the two sides spell the same
    /// version differently. Build metadata is ignored for precedence (§10), so
    /// a re-published manifest that only gained a `+build` must not surface a
    /// phantom "update available".
    #[test]
    fn equal_versions_are_never_newer() {
        assert!(!is_newer("0.11.0-beta.1", "0.11.0-beta.1"));
        assert!(!is_newer("v0.11.0-beta.1", "0.11.0-beta.1"));
        assert!(!is_newer("1.0.0+build.7", "1.0.0"));
        assert!(!is_newer("1.0.0+build.7", "1.0.0+build.6"));
        assert!(is_newer("1.0.1+build.1", "1.0.0+build.9"));
    }

    #[test]
    fn is_newer_falls_back_to_string_diff_for_non_semver() {
        assert!(is_newer("2026.05.31", "2026.05.30"));
        assert!(!is_newer("nightly", "nightly"));
        // A malformed prerelease has no defined ordering, so it falls back to
        // "different strings only" rather than inventing one.
        assert!(!is_newer("1.0.0-", "1.0.0-"));
    }

    // ── Conformance suite ────────────────────────────────────────────────────
    //
    // Written against the HAND-ROLLED parser, BEFORE swapping it for the
    // `semver` crate, so the swap has something to be measured against rather
    // than merely re-asserted by tests written to match its outcome. Every
    // assertion below passes on both implementations EXCEPT the two explicitly
    // marked `PINS TODAY'S BEHAVIOUR` — those record where the swap
    // deliberately changes what `is_newer` answers, so the change shows up as a
    // line in the diff instead of as a surprise in the fleet.

    /// `v` is stripped repeatedly, not once. Nothing produces `vv1.2.3`
    /// deliberately, but a tag-to-version step that prefixes an already
    /// prefixed tag would, and the answer must not silently become "any
    /// difference is newer".
    #[test]
    fn a_repeated_v_prefix_is_still_stripped() {
        assert!(is_newer("vv1.2.4", "1.2.3"));
        assert!(!is_newer("vv1.2.3", "v1.2.3"));
        assert!(!is_newer("v1.2.3", "vv1.2.3"));
    }

    /// Build metadata is ignored for precedence (§10) — in BOTH directions.
    ///
    /// This is the assertion that stops the guard from degenerating into the
    /// updater plugin's own test: `tauri-plugin-updater` 2.10.1 decides with
    /// `release.version > self.current_version`, and `semver::Version`'s `Ord`
    /// compares build metadata so that the type has a total order. A feed that
    /// re-published `1.0.0` as `1.0.0+build.7` therefore looks like an update
    /// TO THE PLUGIN. This guard is what makes it not one.
    #[test]
    fn build_metadata_is_ignored_for_precedence_in_both_directions() {
        assert!(!is_newer("1.0.0+build.7", "1.0.0+build.6"));
        assert!(!is_newer("1.0.0+build.6", "1.0.0+build.7"));
        assert!(!is_newer("1.0.0+build.7", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.0.0+build.7"));
        // Metadata is ignored, not weighted: the core still decides.
        assert!(is_newer("1.0.1+build.1", "1.0.0+build.9"));
        assert!(!is_newer("1.0.0+build.9", "1.0.1+build.1"));
        // …and it is ignored on prereleases too.
        assert!(!is_newer("1.0.0-beta.1+a", "1.0.0-beta.1+b"));
        assert!(is_newer("1.0.0-beta.2+a", "1.0.0-beta.1+z"));
    }

    /// Double-digit prerelease numbers, from both sides. As strings
    /// `"10" < "9"`, so a lexical comparator freezes the beta ring at the
    /// ninth beta of a version — the near-miss this module already documents.
    #[test]
    fn double_digit_prereleases_order_numerically() {
        assert!(is_newer("0.11.0-beta.10", "0.11.0-beta.9"));
        assert!(is_newer("0.11.0-beta.10", "0.11.0-beta.2"));
        assert!(!is_newer("0.11.0-beta.9", "0.11.0-beta.10"));
        assert!(!is_newer("0.11.0-beta.2", "0.11.0-beta.10"));
    }

    /// A prerelease never supersedes the release it was testing for — bug #95
    /// in the opposite direction. A stable install must never be offered a
    /// beta of the version it is already running.
    #[test]
    fn a_prerelease_never_supersedes_its_own_release() {
        assert!(!is_newer("1.2.3-beta.1", "1.2.3"));
        assert!(!is_newer("1.2.3-rc.9", "1.2.3"));
        assert!(!is_newer("1.2.3-alpha", "1.2.3"));
        // …but a prerelease of a HIGHER core still is newer (that is how the
        // beta ring gets ahead of stable in the first place).
        assert!(is_newer("1.2.4-alpha", "1.2.3"));
    }

    /// Every release this project has actually cut, in order, must form a
    /// strictly increasing chain: each tag newer than its predecessor, never
    /// the reverse, and never newer than itself. This is `docs/ROLLBACK.md`'s
    /// "a client only ever moves to a HIGHER version" stated as an executable
    /// property over the real tag history rather than a prose claim.
    #[test]
    fn the_real_release_history_is_strictly_monotone() {
        // `git tag --list 'v0.*'`, in release order.
        let tags = [
            "v0.2.0",
            "v0.3.0",
            "v0.3.1",
            "v0.4.0",
            "v0.4.1",
            "v0.4.2",
            "v0.4.3",
            "v0.4.4",
            "v0.4.5",
            "v0.4.6",
            "v0.5.0",
            "v0.5.1",
            "v0.6.0",
            "v0.7.0",
            "v0.8.0",
            "v0.8.1",
            "v0.9.0",
            "v0.10.0",
            "v0.11.0-beta.1",
            "v0.11.1-beta.2",
            "v0.12.0",
            "v0.13.0",
        ];
        for (i, newer) in tags.iter().enumerate() {
            assert!(!is_newer(newer, newer), "{newer} is not newer than itself");
            for older in &tags[..i] {
                assert!(is_newer(newer, older), "{newer} must supersede {older}");
                assert!(
                    !is_newer(older, newer),
                    "{older} must NOT be offered to a client on {newer}"
                );
            }
        }
    }

    /// PINS TODAY'S BEHAVIOUR — flips with the `semver`-crate swap.
    ///
    /// The hand-rolled parser accepts a zero-padded date tag (`u64::from_str`
    /// is happy with `"05"`) and orders it numerically, so a downgrade is
    /// refused. The `semver` crate rejects the leading zero (§2 forbids it),
    /// which sends the pair to the string-difference fallback — and there,
    /// "the strings differ" means "newer". See the swap commit for why that is
    /// an acceptable loss.
    #[test]
    fn a_zero_padded_date_tag_orders_numerically_today() {
        assert!(!is_newer("2026.05.30", "2026.05.31"));
        assert!(is_newer("2026.05.31", "2026.05.30"));
    }

    /// PINS TODAY'S BEHAVIOUR — flips with the `semver`-crate swap.
    ///
    /// The hand-rolled parser does not trim, so a version that arrives with
    /// the build environment's stray whitespace fails to parse, falls to the
    /// string-difference fallback, and reports a PHANTOM UPDATE to the same
    /// version the machine is already running. `channel_feed_url` already
    /// trims its input for exactly this reason; this half never did.
    #[test]
    fn surrounding_whitespace_produces_a_phantom_update_today() {
        assert!(is_newer(" 1.2.3 ", "1.2.3"));
        assert!(is_newer("1.2.3", " 1.2.3 "));
    }

    #[test]
    fn each_channel_gets_its_own_feed() {
        // These two strings are the live contract with the Worker — a change
        // here is a change to what every shipped client asks for.
        assert_eq!(
            channel_feed_url(DEFAULT_UPDATE_BASE, UpdateChannel::Stable),
            "https://updates.sundaysuite.app/v1/update/stable"
        );
        assert_eq!(
            channel_feed_url(DEFAULT_UPDATE_BASE, UpdateChannel::Beta),
            "https://updates.sundaysuite.app/v1/update/beta"
        );
        // The two rings must never resolve to the same URL.
        assert_ne!(
            channel_feed_url(DEFAULT_UPDATE_BASE, UpdateChannel::Stable),
            channel_feed_url(DEFAULT_UPDATE_BASE, UpdateChannel::Beta)
        );
    }

    #[test]
    fn the_base_is_normalised_before_the_path_is_appended() {
        assert_eq!(
            channel_feed_url("https://updates.sundaysuite.app/", UpdateChannel::Stable),
            "https://updates.sundaysuite.app/v1/update/stable",
            "a trailing slash must not produce //v1"
        );
        assert_eq!(
            channel_feed_url("  http://127.0.0.1:8787  ", UpdateChannel::Beta),
            "http://127.0.0.1:8787/v1/update/beta",
            "the wrangler-dev override arrives with the build env's whitespace"
        );
    }

    #[test]
    fn the_default_base_is_not_the_telemetry_host() {
        // An update check happens with telemetry consent OFF. If this ever reads
        // "telemetry", a declining install appears to call the telemetry
        // endpoint on every launch.
        assert!(!DEFAULT_UPDATE_BASE.contains("telemetry"));
        assert!(DEFAULT_UPDATE_BASE.starts_with("https://"));
    }
}
