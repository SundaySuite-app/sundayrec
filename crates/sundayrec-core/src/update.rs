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

/// Normalise a version string into something [`semver::Version::parse`] will
/// accept: drop surrounding whitespace and any leading `v`.
///
/// `Version::parse` is strict on purpose and rejects both — `"v1.2.3"` and
/// `" 1.2.3"` are hard parse errors, not lenient successes. Without this step
/// a `v`-prefixed tag would fall straight through to the string-difference
/// fallback below, where "the strings differ" means "newer": `v1.2.3` would be
/// offered to a machine already running `1.2.3`. The whitespace half is the
/// same defence `channel_feed_url` already applies to its base URL, for the
/// same reason — a build-env value arrives with the build env's spaces.
///
/// The `v` strip is repeated (`trim_start_matches`), matching what the
/// hand-written parser did, so `vv1.2.3` cannot degrade into a fallback either.
fn normalise(v: &str) -> &str {
    v.trim().trim_start_matches('v')
}

/// Whether `candidate` is strictly newer than `current` under semver ordering.
///
/// The updater plugin already gates on this server-side, but we re-check so a
/// misconfigured feed (e.g. a re-published same-version `latest.json`) never
/// surfaces a phantom "update available" to the user. Strings that are not
/// valid semver fall back to a byte comparison (see below for what that costs).
///
/// This re-check OVERRIDES the plugin: `src-tauri/src/update/mod.rs` turns a
/// `false` here into `UpToDate` no matter what the feed offered. So a version
/// pair this function gets wrong is a version pair no client can ever cross.
/// `docs/ROLLBACK.md` leans on the same property in the other direction ("a
/// client only ever moves to a HIGHER version"), so this must stay strictly
/// monotone: never `true` for an equal or lower candidate.
///
/// ## Why `cmp_precedence` and not `>`
///
/// `semver::Version` implements `Ord` as a TOTAL order, which means it also
/// compares build metadata — `1.0.0+build.7 > 1.0.0+build.6` under `>`. Semver
/// §10 says the opposite: build metadata is ignored for precedence, so those
/// two are the same release with different bytes. `cmp_precedence` is the
/// crate's spec-faithful comparison and the only correct one here; using `>`
/// would reintroduce exactly the phantom update this guard exists to stop.
///
/// This is not hypothetical. `tauri-plugin-updater` 2.10.1 decides with
/// `release.version > self.current_version` — the `Ord` one. A feed that
/// re-published the same release with a `+build` bump is an update TO THE
/// PLUGIN; this function is what makes it not one. The guard is deliberately
/// STRICTER than the thing it guards.
///
/// ## The history this replaces
///
/// The comparison used to be hand-written here (~110 lines implementing §10–11
/// directly). Two things went wrong in it, both worth remembering because the
/// class of bug outlives the code:
///
/// * It compared only `(major, minor, patch)`, so `0.11.0-beta.1` and
///   `0.11.0` were EQUAL. A machine promoted onto a beta could never reach the
///   stable release the beta was testing for — the beta ring was a one-way
///   door, and the app said "Du er oppdatert" the whole time (bug #95).
/// * Numeric prerelease fields were nearly compared as strings, where
///   `"10" < "9"` — which would have frozen the ring at the ninth beta of a
///   version, long after anyone would think to suspect the comparator.
///
/// Both are now the `semver` crate's problem rather than ours, which is the
/// point of the swap: these are the errors a from-scratch reimplementation of a
/// small, fiddly, fully-specified format makes, and they are invisible until a
/// specific version pair in the field cannot be crossed.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    use semver::Version;

    match (
        Version::parse(normalise(candidate)),
        Version::parse(normalise(current)),
    ) {
        (Ok(c), Ok(cur)) => c.cmp_precedence(&cur) == core::cmp::Ordering::Greater,
        // If either side isn't valid semver, only treat as newer when the
        // strings genuinely differ (avoids a same-string false positive).
        //
        // This branch is unreachable from the live call site and is kept as a
        // total-function backstop rather than a supported input path: in
        // `src-tauri/src/update/mod.rs` `current` is
        // `app.package_info().version.to_string()` and `candidate` is the
        // plugin's `Update.version`, and BOTH are `semver::Version::to_string()`
        // output — canonical by construction. Anything reaching here has no
        // defined ordering, and "the strings differ" is a guess. It is the
        // documented, pre-existing guess; see `docs/ROLLBACK.md` §"Versjoner
        // som ikke er streng semver" for what it costs.
        _ => candidate != current,
    }
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
    ///
    /// The `semver` crate is what guarantees this now rather than a
    /// hand-written `PreIdent::Numeric`, but the assertion stays: it is the
    /// property the beta ring depends on, and a test that only holds "because
    /// the dependency is correct" is exactly the test you want when the
    /// dependency is later swapped, pinned, or vendored.
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

    /// PINNED, CHANGED BY THE SWAP — a zero-padded date tag no longer orders.
    ///
    /// The hand-rolled parser accepted `"2026.05.31"` (`u64::from_str` is happy
    /// with `"05"`) and ordered it numerically, so a downgrade was refused.
    /// The `semver` crate rejects the leading zero — semver.org §2 forbids it —
    /// so the pair falls to the string-difference fallback, where "the strings
    /// differ" means "newer". A date-shaped feed could therefore offer a
    /// DOWNGRADE.
    ///
    /// This is a deliberate, argued loss, not an oversight. It cannot happen:
    ///
    /// * `current` is `app.package_info().version` — Cargo refuses to build a
    ///   package whose version has a leading zero at all ("invalid leading zero
    ///   in minor version number"), so no shipped binary can report one.
    /// * `candidate` is the plugin's `Update.version`, produced by
    ///   `RemoteRelease.version.to_string()` where the field is already a
    ///   `semver::Version` — a manifest the crate cannot parse never reaches
    ///   this function.
    /// * No tag in this repository's history is date-shaped
    ///   (`git tag --list`: `v0.2.0`…`v0.13.0`).
    ///
    /// Rescuing it would mean keeping a second, hand-written numeric-triple
    /// parser beside the crate's — two parsers that agree today and disagree
    /// at some later seam, which is the exact failure shape this codebase has
    /// been bitten by repeatedly. One parser, one answer, documented edge.
    /// `docs/ROLLBACK.md` states the rule for operators.
    #[test]
    fn a_zero_padded_date_tag_is_not_semver_and_falls_back() {
        assert!(is_newer("2026.05.30", "2026.05.31"));
        assert!(is_newer("2026.05.31", "2026.05.30"));
        // The un-padded form IS valid semver, and orders correctly. A date tag
        // is only outside the guard when it is zero-padded.
        assert!(!is_newer("2026.5.30", "2026.5.31"));
        assert!(is_newer("2026.5.31", "2026.5.30"));
    }

    /// PINNED, CHANGED BY THE SWAP — whitespace no longer fabricates an update.
    ///
    /// The hand-rolled parser did not trim, so a version arriving with the
    /// build environment's stray whitespace failed to parse, fell to the
    /// string-difference fallback, and reported a PHANTOM UPDATE to the version
    /// the machine was already running. `channel_feed_url` has always trimmed
    /// its input for exactly this reason; this half never did until the wrapper.
    #[test]
    fn surrounding_whitespace_no_longer_fabricates_an_update() {
        assert!(!is_newer(" 1.2.3 ", "1.2.3"));
        assert!(!is_newer("1.2.3", " 1.2.3 "));
        assert!(!is_newer("\t v1.2.3 \n", "1.2.3"));
        // …and it does not swallow a real difference either.
        assert!(is_newer(" 1.2.4 ", "1.2.3"));
    }

    /// The guard must be STRICTER than the plugin it guards, and this is the
    /// assertion that proves the difference is real rather than incidental:
    /// `semver::Version`'s `Ord` (what `tauri-plugin-updater` uses, and what a
    /// naive `c > cur` in `is_newer` would use) ranks `+build.7` above
    /// `+build.6`; `cmp_precedence` — semver §10 — does not.
    ///
    /// If this test ever goes red with a message about build metadata, someone
    /// has replaced `cmp_precedence` with `>` and every `+build` re-publish is
    /// now an update offer.
    #[test]
    fn the_wrapper_uses_precedence_ordering_not_the_total_order() {
        use semver::Version;

        let hi = Version::parse("1.0.0+build.7").unwrap();
        let lo = Version::parse("1.0.0+build.6").unwrap();

        // The total order (`Ord`) DOES separate them — this is the trap.
        assert!(hi > lo, "semver::Version's Ord compares build metadata");
        // Precedence does not, and precedence is what `is_newer` must use.
        assert_eq!(hi.cmp_precedence(&lo), core::cmp::Ordering::Equal);
        assert!(
            !is_newer("1.0.0+build.7", "1.0.0+build.6"),
            "build metadata must never make a version newer (semver §10)"
        );
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

/// The pre-swap comparison, kept as a DIFFERENTIAL ORACLE — test-only, never
/// compiled into a shipped binary.
///
/// This is verbatim the hand-written implementation of semver.org §10–11 that
/// `is_newer` used before it delegated to the `semver` crate. It survives for
/// one reason: it is the only way to prove the swap did not change any answer
/// that mattered. `proptests::the_swap_preserves_every_ordering_that_parsed`
/// runs both over generated version pairs and requires them to agree wherever
/// both sides are valid semver.
///
/// It is NOT a fallback, NOT reachable from `is_newer`, and must never grow a
/// caller. If it ever disagrees with the crate on strict input, the crate is
/// right and this is the bug — that asymmetry is the entire point of having
/// stopped maintaining it.
#[cfg(test)]
mod legacy_reference {
    /// One dot-separated field of a prerelease tag.
    ///
    /// The variant ORDER is load-bearing: the derived `Ord` orders by variant
    /// first, and §11 requires a purely numeric identifier to rank BELOW an
    /// alphanumeric one (`1.0.0-1 < 1.0.0-alpha`).
    #[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
    enum PreIdent {
        Numeric(u64),
        Alpha(String),
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SemVer {
        major: u64,
        minor: u64,
        patch: u64,
        pre: Vec<PreIdent>,
    }

    impl Ord for SemVer {
        fn cmp(&self, other: &Self) -> core::cmp::Ordering {
            use core::cmp::Ordering;

            (self.major, self.minor, self.patch)
                .cmp(&(other.major, other.minor, other.patch))
                .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    (false, false) => self.pre.cmp(&other.pre),
                })
        }
    }

    impl PartialOrd for SemVer {
        fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    fn parse_semver(v: &str) -> Option<SemVer> {
        let s = v.trim_start_matches('v');
        let s = s.split('+').next().unwrap_or(s);

        let (core, pre_str) = match s.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (s, None),
        };

        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }

        let pre = match pre_str {
            None => Vec::new(),
            Some(p) => {
                let mut fields = Vec::new();
                for ident in p.split('.') {
                    if ident.is_empty() {
                        return None;
                    }
                    if ident.bytes().all(|b| b.is_ascii_digit()) {
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

    /// The pre-swap `is_newer`, byte for byte.
    pub(super) fn is_newer(candidate: &str, current: &str) -> bool {
        match (parse_semver(candidate), parse_semver(current)) {
            (Some(c), Some(cur)) => c > cur,
            _ => candidate != current,
        }
    }

    /// Whether the OLD parser considered this string a version — the thing the
    /// differential proptest compares against `semver::Version::parse`, since
    /// every behavioural difference between the two implementations is
    /// ultimately a disagreement about what counts as a version.
    pub(super) fn parses(v: &str) -> bool {
        parse_semver(v).is_some()
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Numeric core fields, plus the surface a real tag varies on: an optional
    /// prerelease of one or two identifiers, optional build metadata, an
    /// optional `v`, and stray whitespace.
    fn version_like(leading_zeros: bool) -> impl Strategy<Value = String> {
        let num: BoxedStrategy<String> = if leading_zeros {
            prop_oneof![
                3 => (0u64..40).prop_map(|n| n.to_string()),
                1 => (0u64..10).prop_map(|n| format!("0{n}")),
            ]
            .boxed()
        } else {
            (0u64..40).prop_map(|n| n.to_string()).boxed()
        };

        let ident: BoxedStrategy<String> = {
            let base = prop_oneof![
                4 => prop_oneof![
                    Just("alpha".to_string()),
                    Just("beta".to_string()),
                    Just("rc".to_string()),
                    // A prerelease identifier may itself contain a hyphen —
                    // `1.2.3-beta-1` is ONE identifier, not a core plus two.
                    Just("beta-1".to_string()),
                ],
                3 => (0u64..15).prop_map(|n| n.to_string()),
            ];
            if leading_zeros {
                prop_oneof![7 => base, 1 => Just("01".to_string())].boxed()
            } else {
                base.boxed()
            }
        };

        (
            num.clone(),
            num.clone(),
            num,
            proptest::option::of(proptest::collection::vec(ident, 1..3)),
            proptest::option::of(prop_oneof![
                Just("build.1".to_string()),
                Just("build.2".to_string())
            ]),
            prop_oneof![8 => Just(""), 1 => Just("v"), 1 => Just(" ")],
        )
            .prop_map(|(ma, mi, pa, pre, build, prefix)| {
                let mut s = format!("{prefix}{ma}.{mi}.{pa}");
                if let Some(pre) = pre {
                    s.push('-');
                    s.push_str(&pre.join("."));
                }
                if let Some(build) = build {
                    s.push('+');
                    s.push_str(&build);
                }
                s
            })
    }

    /// Valid semver by construction (after [`normalise`]) — no leading zeros.
    fn strict_version() -> impl Strategy<Value = String> {
        version_like(false)
    }

    /// Includes the shapes the hand-written parser accepted and the real
    /// grammar does not: `1.0.01`, `1.0.0-beta.01`.
    fn lenient_version() -> impl Strategy<Value = String> {
        version_like(true)
    }

    /// Whether both implementations agree that this string IS a version.
    ///
    /// Every behavioural difference between the old comparison and the new one
    /// reduces to this: they disagree about what counts as a version, and
    /// whichever one rejects falls to the string-difference branch. The two
    /// known disagreements, in both directions:
    ///
    /// * the old parser accepted leading zeros (`1.0.01`, `1.0.0-beta.01`) and
    ///   other strings outside the real grammar; the crate rejects them.
    /// * the old parser did not trim, so ` 1.2.3` was not a version to it; the
    ///   wrapper normalises first, so it is one now. (That direction is a fix:
    ///   it is what stopped whitespace fabricating an update.)
    fn both_parsers_accept(s: &str) -> bool {
        semver::Version::parse(normalise(s)).is_ok() == legacy_reference::parses(s)
    }

    proptest! {
        /// THE swap proof: wherever the two implementations agree on what is a
        /// version, they agree on the ORDER — the crate-backed `is_newer`
        /// answers exactly what the hand-written one answered. The oracle is
        /// `legacy_reference`, which is the pre-swap code verbatim.
        #[test]
        fn the_swap_preserves_every_ordering_that_parsed(
            a in strict_version(),
            b in strict_version(),
        ) {
            prop_assume!(both_parsers_accept(&a) && both_parsers_accept(&b));

            prop_assert_eq!(
                is_newer(&a, &b),
                legacy_reference::is_newer(&a, &b),
                "new and old disagree on the version pair ({}, {})",
                a,
                b
            );
        }

        /// …and every divergence that DOES exist is explained by that
        /// disagreement, never by the ordering rules themselves.
        ///
        /// This runs over the LENIENT generator — leading zeros included — and
        /// does not demand agreement there. It demands that any disagreement
        /// can only ever be attributed to one parser accepting a string the
        /// other rejects. A divergence on two strings both parsers call valid
        /// would mean the swap changed semver §10–11 ordering, and fails here.
        #[test]
        fn every_divergence_is_a_disagreement_about_what_a_version_is(
            a in lenient_version(),
            b in lenient_version(),
        ) {
            if is_newer(&a, &b) != legacy_reference::is_newer(&a, &b) {
                prop_assert!(
                    !both_parsers_accept(&a) || !both_parsers_accept(&b),
                    "both parsers accept ({}, {}) yet the two implementations order them differently",
                    a,
                    b
                );
            }
        }

        /// The property `docs/ROLLBACK.md` depends on: strict monotonicity over
        /// real versions. Nothing is newer than itself, and two versions are
        /// never each newer than the other — otherwise "a client only ever
        /// moves to a HIGHER version" is a coin flip and a fleet can be walked
        /// backwards.
        ///
        /// Stated over STRICT semver on purpose. The string-difference fallback
        /// is not an order and never was (`"0.0.0"` vs `"00.0.0"` is newer in
        /// both directions, in the old implementation exactly as in the new);
        /// `is_newer_on_unparseable_input_is_not_an_order` pins that separately
        /// so the limit is recorded rather than hidden behind an assumption.
        #[test]
        fn is_newer_is_irreflexive_and_antisymmetric(
            a in strict_version(),
            b in strict_version(),
        ) {
            prop_assume!(
                semver::Version::parse(normalise(&a)).is_ok()
                    && semver::Version::parse(normalise(&b)).is_ok()
            );

            prop_assert!(!is_newer(&a, &a), "{} was newer than itself", a);
            prop_assert!(
                !(is_newer(&a, &b) && is_newer(&b, &a)),
                "{} and {} are each newer than the other",
                a,
                b
            );
        }

        /// Never panics. `candidate` is whatever the update feed said, and a
        /// panic in this guard is a crash on every update check the install
        /// ever makes.
        #[test]
        fn is_newer_never_panics(a in ".{0,60}", b in ".{0,60}") {
            let _ = is_newer(&a, &b);
        }
    }

    /// The fallback's limit, stated once and out loud: for input the semver
    /// grammar rejects, `is_newer` degrades to "the strings differ", which is
    /// not an ordering — it can be true in both directions.
    ///
    /// This is UNCHANGED by the swap (the old implementation is asserted right
    /// beside it, agreeing), and unreachable from the live call site, where
    /// both arguments are `semver::Version::to_string()` output. It is pinned
    /// so nobody rediscovers it in a release postmortem.
    #[test]
    fn is_newer_on_unparseable_input_is_not_an_order() {
        // Neither parser calls these versions, so both fall to "the strings
        // differ" — which is true in both directions at once.
        assert!(is_newer("nightly", "main"));
        assert!(is_newer("main", "nightly"));
        assert!(legacy_reference::is_newer("nightly", "main"));
        assert!(legacy_reference::is_newer("main", "nightly"));
        // The zero-padded date pair is the same shape, and the same in both
        // implementations for the candidate direction — what the swap changed
        // is only that the OTHER direction stopped being refused.
        assert!(is_newer("2026.05.31", "2026.05.30"));
        assert!(legacy_reference::is_newer("2026.05.31", "2026.05.30"));
    }
}
