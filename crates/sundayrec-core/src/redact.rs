//! Scrubbing text that is about to be written somewhere it can be read later.
//!
//! Two different worries, one module, both pure:
//!
//!   - [`scrub_paths`] removes the operator's identity from absolute filesystem
//!     paths. A panic backtrace names every source file it walked through, and
//!     on a machine built from source those are all under `/Users/<name>/`. The
//!     crash ring is written to disk so a later phase can offer to upload it, so
//!     the name has to go BEFORE it lands in the file, not before it is sent.
//!   - [`redact_secrets`] removes credentials from a log line. The file log is
//!     the one artefact a volunteer operator will cheerfully paste into a chat
//!     channel; a stream key or an SMTP password in it is a leak with a very
//!     short fuse.
//!
//! Both are deliberately conservative: they may over-redact (a directory called
//! `token` loses its value), never under-redact. The cost of a false positive is
//! one unreadable log line; the cost of a false negative is a credential in a
//! chat channel.
//!
//! The discipline is the one `streaming::loggable_args` already applies to the
//! ffmpeg argv (a parallel "loggable" copy with keys replaced by `***`) —
//! generalised here so every OTHER formatted string gets the same treatment.

/// Path roots whose FIRST segment is a user name. Matched case-insensitively so
/// a Windows path that came back from an API as `C:\USERS\Ola\…` is caught too.
const USER_ROOTS: &[&str] = &["/Users/", "/home/", "\\Users\\"];

/// What a user name is replaced with. Deliberately not empty: a reader should
/// see that a name WAS there and was removed, rather than wonder about a
/// malformed path.
const USER_PLACEHOLDER: &str = "<user>";

/// Replace absolute user paths in `text` so it carries no operator identity.
///
/// Two passes, in this order:
///
///   1. `home` (when given) is replaced with `~`, longest form first, so the
///      common case reads naturally: `/Users/ola/Documents/x` → `~/Documents/x`.
///   2. Any remaining `/Users/<seg>`, `/home/<seg>` or `\Users\<seg>` has its
///      `<seg>` replaced with `<user>` — this catches the paths that are NOT
///      under this user's home (a backtrace from a crate built in someone
///      else's checkout, a second account's folder).
///
/// Idempotent: `<user>` contains no path separator, so a second pass sees the
/// placeholder as the segment and rewrites it to itself.
pub fn scrub_paths(text: &str, home: Option<&str>) -> String {
    let mut out = match home.map(str::trim).filter(|h| !h.is_empty()) {
        // Strip a trailing separator first so `/Users/ola/` and `/Users/ola`
        // both collapse to the same `~` and we do not leave a `~/` + `/`.
        Some(home) => {
            let trimmed = home.trim_end_matches(['/', '\\']);
            if trimmed.is_empty() {
                text.to_string()
            } else {
                text.replace(trimmed, "~")
            }
        }
        None => text.to_string(),
    };
    out = scrub_user_roots(&out);
    out
}

/// The second pass of [`scrub_paths`]: rewrite the first segment after a known
/// user root. Split out so it can be tested on its own.
fn scrub_user_roots(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < text.len() {
        // The earliest root marker at or after `i`, if any.
        let hit = USER_ROOTS
            .iter()
            .filter_map(|root| {
                lower[i..]
                    .find(&root.to_ascii_lowercase())
                    .map(|at| (i + at, root.len()))
            })
            .min_by_key(|(at, _)| *at);
        let Some((at, root_len)) = hit else {
            out.push_str(&text[i..]);
            break;
        };
        // Everything up to and including the root marker is copied verbatim so
        // the original casing/separator survives.
        out.push_str(&text[i..at + root_len]);
        let rest = &text[at + root_len..];
        // The user segment runs to the next separator (or the end of the text).
        let seg_end = rest.find(['/', '\\']).unwrap_or(rest.len());
        if seg_end == 0 {
            // `/Users//x` or a trailing `/Users/` — nothing to redact.
            i = at + root_len;
            continue;
        }
        out.push_str(USER_PLACEHOLDER);
        i = at + root_len + seg_end;
    }
    out
}

/// Key names whose VALUE is a credential. Matched case-insensitively against the
/// token immediately left of an `=` or `:` separator, so both `key=abc` and
/// `"stream_key": "abc"` are caught.
const SECRET_KEYS: &[&str] = &[
    "access_token",
    "refresh_token",
    "id_token",
    "client_secret",
    "authorization",
    "api_key",
    "apikey",
    "stream_key",
    "streamkey",
    "password",
    "passwd",
    "secret",
    "token",
    "key",
    "bearer",
];

/// What a redacted value is replaced with — the same marker
/// `streaming::loggable_args` already uses, so a reader sees one convention.
pub const SECRET_PLACEHOLDER: &str = "***";

/// Remove credentials from one line of text before it reaches the log file.
///
/// Three shapes are handled, which between them cover everything this codebase
/// can format:
///
///   - `key=VALUE` / `key: VALUE` / `"key": "VALUE"` for any name in
///     [`SECRET_KEYS`] (case-insensitive, `-`/`_` equivalent);
///   - `Bearer <token>` in an Authorization header;
///   - the last path segment of an `rtmp://`/`rtmps://` URL, which for every
///     streaming destination in existence IS the stream key.
///
/// Conservative by design: the *name* is what triggers the redaction, so a
/// value that merely looks secret is left alone (unreadable logs help nobody),
/// but anything sitting under a secret-shaped name goes, whatever it contains.
pub fn redact_secrets(line: &str) -> String {
    let out = redact_rtmp_keys(line);
    redact_keyed_values(&out)
}

/// Whether `name` (already lowercased, `-` folded to `_` by [`name_left_of`]) is
/// a credential name. Whole-name only: `monkey` is not a `key`.
fn is_secret_key(name: &str) -> bool {
    SECRET_KEYS.contains(&name)
}

/// Auth schemes that sit BETWEEN a secret name and its value
/// (`Authorization: Bearer <token>`), so the token is one word further right
/// than the naive "value = next word" rule would find.
const AUTH_SCHEMES: &[&str] = &["bearer", "basic", "token"];

/// `…key=VALUE…` / `…"key": "VALUE"…` → `…key=***…`.
///
/// Works on BYTES, not chars: every delimiter it cuts at (`=`, `:`, space,
/// quote, `,`, `&`, `}`) is ASCII, and no multi-byte UTF-8 sequence contains an
/// ASCII byte — so copying byte runs can never split a character. That keeps the
/// scanner simple AND makes a Norwegian device name in a log line safe to carry.
fn redact_keyed_values(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'=' && b != b':' {
            out.push(b);
            i += 1;
            continue;
        }
        if !is_secret_key(&name_left_of(&out)) {
            out.push(b);
            i += 1;
            continue;
        }
        // A secret name: keep the separator + any spacing/opening quote, then
        // swallow the value.
        out.push(b);
        i += 1;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            out.push(b' ');
            i += 1;
        }
        let quote = match bytes.get(i) {
            Some(&q @ (b'"' | b'\'')) => {
                out.push(q);
                i += 1;
                Some(q)
            }
            _ => None,
        };
        // `Authorization: Bearer <tok>` — the scheme word is not the secret, so
        // step over it (and its space) and redact what follows instead.
        if quote.is_none() {
            let word_end = value_end(bytes, i, None);
            let word = String::from_utf8_lossy(&bytes[i..word_end]).to_ascii_lowercase();
            if AUTH_SCHEMES.contains(&word.as_str()) && word_end < bytes.len() {
                i = word_end;
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
            }
        }
        let value_start = i;
        i = value_end(bytes, i, quote);
        if i > value_start {
            out.extend_from_slice(SECRET_PLACEHOLDER.as_bytes());
        }
        if let Some(q) = quote {
            if i < bytes.len() {
                out.push(q);
                i += 1;
            }
        }
    }
    // Every byte copied came from a valid UTF-8 string and was never split, so
    // this cannot fail; `lossy` is the belt that keeps the signature infallible.
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Where the value starting at `from` ends: at the closing `quote` when quoted,
/// otherwise at the first structural delimiter.
fn value_end(bytes: &[u8], from: usize, quote: Option<u8>) -> usize {
    let mut i = from;
    while i < bytes.len() {
        let b = bytes[i];
        let ends = match quote {
            Some(q) => b == q,
            None => matches!(b, b' ' | b'\t' | b',' | b'&' | b'}' | b'"' | b'\''),
        };
        if ends {
            break;
        }
        i += 1;
    }
    i
}

/// The identifier immediately left of the separator at the end of `out`,
/// lowercased with `-` folded to `_`. Empty when there is none.
fn name_left_of(out: &[u8]) -> String {
    let mut end = out.len();
    while end > 0 && matches!(out[end - 1], b' ' | b'\t' | b'"' | b'\'') {
        end -= 1;
    }
    let mut start = end;
    while start > 0 {
        let b = out[start - 1];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            start -= 1;
        } else {
            break;
        }
    }
    String::from_utf8_lossy(&out[start..end])
        .to_ascii_lowercase()
        .replace('-', "_")
}

/// `rtmp://host/app/STREAMKEY` → `rtmp://host/app/***`. The trailing segment of
/// an RTMP ingest URL is the key for YouTube, Facebook, Twitch and every other
/// destination the streaming seam supports.
fn redact_rtmp_keys(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut i = 0usize;
    while i < line.len() {
        let Some(rel) = lower[i..].find("rtmp") else {
            out.push_str(&line[i..]);
            break;
        };
        let at = i + rel;
        // Only a real scheme, not the word "rtmp" in prose.
        let scheme_len = if lower[at..].starts_with("rtmps://") {
            8
        } else if lower[at..].starts_with("rtmp://") {
            7
        } else {
            out.push_str(&line[i..at + 4]);
            i = at + 4;
            continue;
        };
        let url_start = at;
        let rest = &line[url_start..];
        // The URL runs to the first character that cannot be in one.
        let url_len = rest
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',')
            .unwrap_or(rest.len());
        let url = &rest[..url_len];
        out.push_str(&line[i..url_start]);
        match url[scheme_len..].rfind('/') {
            // A key segment exists (and is not empty) — replace it.
            Some(slash) if slash + scheme_len + 1 < url.len() => {
                out.push_str(&url[..scheme_len + slash + 1]);
                out.push_str(SECRET_PLACEHOLDER);
            }
            _ => out.push_str(url),
        }
        i = url_start + url_len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── scrub_paths ──────────────────────────────────────────────────────────

    #[test]
    fn the_home_dir_becomes_a_tilde() {
        assert_eq!(
            scrub_paths(
                "panicked at /Users/ola/dev/sundayrec/src/lib.rs:12",
                Some("/Users/ola")
            ),
            "panicked at ~/dev/sundayrec/src/lib.rs:12"
        );
        // A trailing separator on the home must not leave a doubled slash.
        assert_eq!(
            scrub_paths("/Users/ola/x", Some("/Users/ola/")),
            "~/x",
            "a trailing separator on HOME must collapse with the path's"
        );
    }

    #[test]
    fn a_foreign_user_root_loses_its_name_even_without_a_home() {
        assert_eq!(
            scrub_paths("/Users/someoneelse/.cargo/registry/x.rs", None),
            "/Users/<user>/.cargo/registry/x.rs"
        );
        assert_eq!(scrub_paths("/home/bob/app", None), "/home/<user>/app");
        assert_eq!(
            scrub_paths("C:\\Users\\Ola\\AppData\\Roaming\\x", None),
            "C:\\Users\\<user>\\AppData\\Roaming\\x"
        );
    }

    #[test]
    fn windows_roots_match_case_insensitively() {
        assert_eq!(
            scrub_paths("D:\\USERS\\Kari\\log.txt", None),
            "D:\\USERS\\<user>\\log.txt",
            "the marker keeps its original casing; only the name is replaced"
        );
    }

    #[test]
    fn every_occurrence_is_scrubbed_not_only_the_first() {
        let s = scrub_paths("/home/a/x and /home/b/y and /home/c/z", None);
        assert_eq!(s, "/home/<user>/x and /home/<user>/y and /home/<user>/z");
    }

    #[test]
    fn scrubbing_is_idempotent() {
        let once = scrub_paths("/Users/ola/dev/x.rs", None);
        assert_eq!(scrub_paths(&once, None), once);
        let with_home = scrub_paths("/Users/ola/dev/x.rs", Some("/Users/ola"));
        assert_eq!(scrub_paths(&with_home, Some("/Users/ola")), with_home);
    }

    #[test]
    fn text_without_a_user_path_is_returned_unchanged() {
        for s in [
            "",
            "no paths here",
            "/opt/homebrew/bin/ffmpeg",
            "/var/folders/xy/T/sundayrec-bench/x.wav",
        ] {
            assert_eq!(scrub_paths(s, Some("/Users/ola")), s, "{s}");
        }
    }

    #[test]
    fn a_blank_or_root_home_is_ignored_rather_than_eating_the_text() {
        // A blank HOME must not turn every character into a tilde.
        assert_eq!(scrub_paths("/home/bob/x", Some("")), "/home/<user>/x");
        assert_eq!(scrub_paths("/home/bob/x", Some("   ")), "/home/<user>/x");
        assert_eq!(scrub_paths("/home/bob/x", Some("/")), "/home/<user>/x");
    }

    #[test]
    fn a_bare_root_with_no_segment_is_left_alone() {
        assert_eq!(scrub_paths("see /Users/", None), "see /Users/");
        assert_eq!(scrub_paths("/home//weird", None), "/home//weird");
    }

    // ── redact_secrets ───────────────────────────────────────────────────────

    #[test]
    fn keyed_values_are_replaced_by_name() {
        assert_eq!(redact_secrets("stream_key=abc123"), "stream_key=***");
        assert_eq!(
            redact_secrets("password=hunter2 host=smtp.x"),
            "password=*** host=smtp.x"
        );
        assert_eq!(
            redact_secrets("{\"access_token\": \"ya29.long\", \"expires_in\": 3600}"),
            "{\"access_token\": \"***\", \"expires_in\": 3600}"
        );
        assert_eq!(
            redact_secrets("Authorization: Bearer eyJhbGciOi"),
            "Authorization: ***",
            "the whole header value goes, Bearer included"
        );
        // `-` and `_` are the same name.
        assert_eq!(redact_secrets("api-key=zzz"), "api-key=***");
        // Case-insensitive.
        assert_eq!(redact_secrets("PASSWORD=zzz"), "PASSWORD=***");
    }

    #[test]
    fn innocent_keys_keep_their_values() {
        for s in [
            "device=Qu-5 rate=48000",
            "path=/tmp/x.wav",
            "code=device_disconnected message=Enheten forsvant",
            "monkey=banana",
        ] {
            assert_eq!(redact_secrets(s), s, "{s}");
        }
    }

    #[test]
    fn a_key_shaped_name_inside_a_longer_word_is_not_a_secret() {
        // `monkey` ends in "key" but is not one; the name is taken whole.
        assert_eq!(redact_secrets("monkey=banana"), "monkey=banana");
        assert_eq!(redact_secrets("keyboard=apple"), "keyboard=apple");
    }

    #[test]
    fn an_rtmp_ingest_url_loses_its_trailing_key() {
        assert_eq!(
            redact_secrets("rtmp://a.rtmp.youtube.com/live2/abcd-efgh-ijkl"),
            "rtmp://a.rtmp.youtube.com/live2/***"
        );
        assert_eq!(
            redact_secrets("pushing to rtmps://live.x.com/rtmp/SECRETKEY now"),
            "pushing to rtmps://live.x.com/rtmp/*** now"
        );
        // No key segment → nothing to remove, and no mangling.
        assert_eq!(redact_secrets("rtmp://host/"), "rtmp://host/");
        // The word in prose is not a URL.
        assert_eq!(
            redact_secrets("the rtmp push failed"),
            "the rtmp push failed"
        );
    }

    #[test]
    fn redaction_is_idempotent() {
        for s in [
            "stream_key=abc123",
            "rtmp://a/live2/abcd",
            "{\"refresh_token\": \"x\"}",
        ] {
            let once = redact_secrets(s);
            assert_eq!(redact_secrets(&once), once, "{s}");
        }
    }

    #[test]
    fn a_line_with_nothing_to_hide_is_returned_unchanged() {
        let s = "recorder: spawning ffmpeg segment (48000 Hz, stereo)";
        assert_eq!(redact_secrets(s), s);
        assert_eq!(redact_secrets(""), "");
    }
}
