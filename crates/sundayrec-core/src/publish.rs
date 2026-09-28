//! «Legg ut» — where the volunteer puts the finished file, as pure decisions.
//!
//! SundayRec does not upload anything. The sharing cluster (cloud backup,
//! podcast RSS, OAuth) left in R1 «Frivilligen først», and the file on disk is
//! still the hand-off. What this module decides is the one step AROUND that
//! hand-off the app can make easier: which upload page to open in the system
//! browser once the export is done, so the volunteer only has to drag the file
//! in. The church picks the channel once, in Oppsett
//! ([`crate::settings::Settings::publish_target`]).
//!
//! Every answer is a fixed, well-known `https://` address or the church's own
//! link after [`custom_upload_url`] has vetted it. The webview never gets to
//! name a URL to open: the shell reads the stored setting and asks this module
//! (`publish_open_upload_page`), so the `opener` door stays exactly as wide as
//! this table.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::telemetry::CounterName;

/// The channel «Legg ut» opens. Serialised lowercase; `none` hides the panel.
///
/// SoundCloud is the default because it is what most churches told us they
/// distribute sermons with. The others are the channels they named next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "PublishTarget.ts")]
#[serde(rename_all = "lowercase")]
pub enum PublishTarget {
    /// SoundCloud's upload page. Often also the church's podcast RSS feed.
    #[default]
    Soundcloud,
    /// YouTube's upload flow (it opens YouTube Studio's upload dialog).
    Youtube,
    /// Spotify for Creators — the podcast dashboard, where episodes are added.
    Spotify,
    /// The church's own page ([`crate::settings::Settings::publish_custom_url`]):
    /// a WordPress admin, a podcast host's dashboard, …
    Custom,
    /// No «Legg ut» at all — the receipt shows only the file.
    #[serde(rename = "none")]
    Off,
}

/// SoundCloud's upload page.
pub const SOUNDCLOUD_UPLOAD_URL: &str = "https://soundcloud.com/upload";
/// YouTube's upload entry point (redirects into YouTube Studio's dialog).
pub const YOUTUBE_UPLOAD_URL: &str = "https://www.youtube.com/upload";
/// Spotify for Creators' dashboard.
pub const SPOTIFY_UPLOAD_URL: &str = "https://creators.spotify.com/";

/// The longest custom link we accept. A real upload page is a short address;
/// anything near this is pasted text, not a link.
pub const CUSTOM_URL_MAX_LEN: usize = 2048;

/// The page «Legg ut» opens for `target`, or `None` when there is nothing to
/// open (`Off`, or a custom link that does not pass [`custom_upload_url`]).
pub fn upload_page_url(target: PublishTarget, custom: &str) -> Option<String> {
    match target {
        PublishTarget::Soundcloud => Some(SOUNDCLOUD_UPLOAD_URL.to_string()),
        PublishTarget::Youtube => Some(YOUTUBE_UPLOAD_URL.to_string()),
        PublishTarget::Spotify => Some(SPOTIFY_UPLOAD_URL.to_string()),
        PublishTarget::Custom => custom_upload_url(custom),
        PublishTarget::Off => None,
    }
}

/// The usage counter an opened `target` bumps, or `None` for `Off`.
///
/// One counter per channel and no more: which channels churches actually use
/// is the question, and a closed name per channel answers it without the
/// church's own link (or anything else it typed) ever reaching a payload.
pub fn counter_for(target: PublishTarget) -> Option<CounterName> {
    match target {
        PublishTarget::Soundcloud => Some(CounterName::EditorPublishSoundcloud),
        PublishTarget::Youtube => Some(CounterName::EditorPublishYoutube),
        PublishTarget::Spotify => Some(CounterName::EditorPublishSpotify),
        PublishTarget::Custom => Some(CounterName::EditorPublishCustom),
        PublishTarget::Off => None,
    }
}

/// The church's own upload link, trimmed — or `None` if it is not one we will
/// hand to the operating system.
///
/// **`https://` only.** The link is opened by the OS, not by the webview, and
/// the OS will happily open `file://`, `smb://` or an app's own scheme. A
/// plain `http://` page would put a login form on the wire in clear text.
///
/// **No userinfo.** `https://soundcloud.com@example.net/` READS as SoundCloud
/// and GOES to example.net; an `@` anywhere before the path is refused.
///
/// **One line, no spaces, a real host.** Pasted text with a line break or a
/// space in it is not a link, and `https:///path` has no host to go to.
pub fn custom_upload_url(raw: &str) -> Option<String> {
    let url = raw.trim();
    if url.len() > CUSTOM_URL_MAX_LEN {
        return None;
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let scheme_end = "https://".len();
    // `get`, not `[..]`: byte 8 may sit inside a multi-byte letter
    // (`httpsæ//…`), and slicing there would panic instead of refusing.
    let (Some(scheme), Some(rest)) = (url.get(..scheme_end), url.get(scheme_end..)) else {
        return None;
    };
    if rest.is_empty() || !scheme.eq_ignore_ascii_case("https://") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || authority.contains('\\') {
        return None;
    }
    // A host, optionally `:port` — and the host must have a name in it.
    let host = authority.rsplit_once(':').map_or(authority, |(h, port)| {
        if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
            h
        } else {
            authority
        }
    });
    if host.starts_with('.') || !host.chars().any(|c| c.is_alphanumeric()) {
        return None;
    }
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixed_channels_are_https_and_do_not_need_a_link() {
        for target in [
            PublishTarget::Soundcloud,
            PublishTarget::Youtube,
            PublishTarget::Spotify,
        ] {
            let url = upload_page_url(target, "").expect("a fixed channel always has a page");
            assert!(url.starts_with("https://"), "{target:?} → {url}");
            // …and the custom link is ignored for them.
            assert_eq!(upload_page_url(target, "https://example.org/"), Some(url));
        }
        assert_eq!(
            upload_page_url(PublishTarget::Soundcloud, ""),
            Some("https://soundcloud.com/upload".to_string())
        );
    }

    #[test]
    fn every_channel_that_opens_something_has_its_own_counter() {
        let counters: Vec<_> = [
            PublishTarget::Soundcloud,
            PublishTarget::Youtube,
            PublishTarget::Spotify,
            PublishTarget::Custom,
        ]
        .into_iter()
        .map(|t| counter_for(t).expect("an opening channel is counted"))
        .collect();
        let mut unique = counters.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), counters.len(), "one counter per channel");
        for c in counters {
            assert!(c.as_wire().starts_with("editor.publish."), "{c:?}");
            assert_eq!(CounterName::from_wire(c.as_wire()), Some(c));
        }
        assert_eq!(counter_for(PublishTarget::Off), None);
    }

    #[test]
    fn off_opens_nothing() {
        assert_eq!(
            upload_page_url(PublishTarget::Off, "https://example.org/"),
            None
        );
    }

    #[test]
    fn a_custom_link_is_trimmed_and_used() {
        assert_eq!(
            upload_page_url(
                PublishTarget::Custom,
                "  https://kirken.no/wp-admin/post-new.php  "
            ),
            Some("https://kirken.no/wp-admin/post-new.php".to_string())
        );
        assert_eq!(
            custom_upload_url("HTTPS://Podcast.Example:8443/episodes?new=1#top"),
            Some("HTTPS://Podcast.Example:8443/episodes?new=1#top".to_string())
        );
    }

    #[test]
    fn a_custom_link_that_is_not_an_https_page_is_refused() {
        for bad in [
            "",
            "   ",
            "http://kirken.no/",
            "https://",
            "https:///sti",
            "javascript:alert(1)",
            "file:///Users/test/.ssh/id_rsa",
            "smb://server/share",
            "kirken.no/upload",
            "https://soundcloud.com@example.net/upload",
            "https://user:pass@kirken.no/",
            "https://kirken.no/med mellomrom",
            "https://kirken.no/\nhttps://example.net/",
            "https://\\\\server/share",
            "https://.:443/",
            "https://:443/",
        ] {
            assert_eq!(custom_upload_url(bad), None, "{bad:?}");
            assert_eq!(upload_page_url(PublishTarget::Custom, bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_letter_across_the_scheme_boundary_is_refused_not_a_panic() {
        // «æ» is two bytes; byte 8 falls inside it.
        for bad in ["httpsæ//kirken.no", "https:/æ/kirken.no", "ææææ", "https:æ"] {
            assert_eq!(custom_upload_url(bad), None, "{bad:?}");
        }
    }

    /// The vectors the renderer's `customUrlProblem` is held to as well, so
    /// Oppsett never calls a link fine that this refuses to open (or the
    /// other way round).
    #[test]
    fn custom_upload_url_matches_the_shared_vectors() {
        #[derive(serde::Deserialize)]
        struct Vector {
            url: String,
            ok: bool,
        }
        let vectors: Vec<Vector> =
            serde_json::from_str(include_str!("../tests/fixtures/custom-upload-url.json")).unwrap();
        assert!(vectors.len() >= 15, "the fixture lost its vectors");
        for v in vectors {
            assert_eq!(custom_upload_url(&v.url).is_some(), v.ok, "{:?}", v.url);
        }
    }

    #[test]
    fn a_custom_link_has_a_length_limit() {
        let long = format!("https://kirken.no/{}", "a".repeat(CUSTOM_URL_MAX_LEN));
        assert_eq!(custom_upload_url(&long), None);
    }

    #[test]
    fn the_target_serialises_as_the_lowercase_names_the_renderer_uses() {
        let wire = |t: PublishTarget| serde_json::to_string(&t).unwrap();
        assert_eq!(wire(PublishTarget::Soundcloud), "\"soundcloud\"");
        assert_eq!(wire(PublishTarget::Youtube), "\"youtube\"");
        assert_eq!(wire(PublishTarget::Spotify), "\"spotify\"");
        assert_eq!(wire(PublishTarget::Custom), "\"custom\"");
        assert_eq!(wire(PublishTarget::Off), "\"none\"");
        assert_eq!(PublishTarget::default(), PublishTarget::Soundcloud);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// The link is whatever someone typed or pasted into Oppsett. No input
        /// may panic, and anything accepted must still be an `https://` link on
        /// one line — the only shape the OS is ever handed.
        #[test]
        fn custom_upload_url_never_panics_and_only_passes_https(raw in "\\PC{0,80}") {
            if let Some(url) = custom_upload_url(&raw) {
                prop_assert!(url.to_ascii_lowercase().starts_with("https://"));
                prop_assert!(!url.chars().any(|c| c.is_whitespace() || c.is_control()));
            }
        }
    }
}
