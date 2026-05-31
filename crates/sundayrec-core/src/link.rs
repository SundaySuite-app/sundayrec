//! Sunday Bridge deep-link builders (sender side).
//!
//! Hands a finished recording to a sister Sunday-suite app via its
//! `<scheme>://import?…` URL. Today that's SundayEdit (captioning): a recording
//! flows straight into it with `returnTo=sundayrec` so SundayEdit can hand the
//! captions back. This is the pure, testable builder; the `src-tauri` shell
//! opens the URL via the OS.
//!
//! The grammar mirrors SundayEdit's receiving `deeplink.rs` and the platform
//! `sunday-contracts::deeplink::MediaHandoff` contract: `application/
//! x-www-form-urlencoded` component encoding, but spaces as `%20` (never `+`)
//! so they survive the receiver's `+`→space decode. Once the `sunday-contracts`
//! crate is published (git tag), this should converge onto it.

/// The scheme SundayEdit registers for inbound import links.
pub const SUNDAYEDIT_SCHEME: &str = "sundayedit";
/// The scheme SundayStudio registers (podcast/jingle import).
pub const SUNDAYSTUDIO_SCHEME: &str = "sundaystudio";

/// Build a `<scheme>://import?path=<enc>[&returnTo=<enc>]` deep link to hand a
/// media file to a sister app.
pub fn build_import_url(scheme: &str, path: &str, return_to: Option<&str>) -> String {
    let mut url = format!("{scheme}://import?path={}", encode_component(path));
    if let Some(rt) = return_to.filter(|s| !s.is_empty()) {
        url.push_str("&returnTo=");
        url.push_str(&encode_component(rt));
    }
    url
}

/// Percent-encode a URL query-component value: RFC 3986 unreserved chars pass
/// through, everything else (incl. `/`, spaces, non-ASCII) becomes `%XX`.
/// Spaces always encode as `%20`, never `+`.
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(hex_digit(b >> 4));
                out.push(hex_digit(b & 0x0f));
            }
        }
    }
    out
}

fn hex_digit(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'A' + (n - 10)) as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_sundayedit_import_link() {
        let url = build_import_url(
            SUNDAYEDIT_SCHEME,
            "/Users/ola/Opptak 2026.mp4",
            Some("sundayrec"),
        );
        assert_eq!(
            url,
            "sundayedit://import?path=%2FUsers%2Fola%2FOpptak%202026.mp4&returnTo=sundayrec"
        );
    }

    #[test]
    fn omits_return_to_when_absent_or_empty() {
        assert_eq!(
            build_import_url("sundaystudio", "/a.wav", None),
            "sundaystudio://import?path=%2Fa.wav"
        );
        assert_eq!(
            build_import_url("sundaystudio", "/a.wav", Some("")),
            "sundaystudio://import?path=%2Fa.wav"
        );
    }

    #[test]
    fn spaces_are_percent20_not_plus() {
        let url = build_import_url(SUNDAYEDIT_SCHEME, "/a b.mp4", None);
        assert!(url.contains("%20"));
        assert!(!url.contains('+'));
    }

    #[test]
    fn encodes_non_ascii() {
        // æøå must survive as %XX bytes (the receiver decodes UTF-8 lossily).
        let url = build_import_url(SUNDAYEDIT_SCHEME, "/søndag.mp4", None);
        assert!(url.starts_with("sundayedit://import?path=%2Fs"));
        assert!(!url.contains('ø'));
    }
}
