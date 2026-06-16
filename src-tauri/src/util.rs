//! Small cross-cutting helpers shared across the shell modules.

use std::sync::{Mutex, MutexGuard};

use sundayrec_core::ffmpeg::Platform;

/// The platform we're running on, mapped to the core [`Platform`] enum. A
/// compile-time `cfg!` check, consolidated here so the recorder, preroll, and
/// preview seams stop each carrying an identical copy.
pub fn detect_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOS
    } else {
        Platform::Linux
    }
}

/// Lock a [`Mutex`], recovering its inner value if a previous holder panicked
/// rather than propagating the poison.
///
/// Every mutex in this crate guards plain bookkeeping (a status snapshot, an
/// `Option<JoinHandle>`, a counter) — never an invariant a panic could leave
/// half-broken. So taking the poisoned inner guard is correct, and strictly safer
/// than `.lock().expect(...)`: a single panicked thread must not cascade into a
/// crash on every later lock — least of all mid-recording, the worst possible
/// moment. On the happy path this is identical to `.lock().unwrap()`.
///
/// Consolidated here so the ~9 modules that need it stop each carrying their own
/// copy.
pub fn lock_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Minimal percent-encoding for URL query/path values: keep the RFC-3986
/// unreserved set (`A-Za-z0-9-._~`) verbatim, `%XX`-encode every other byte.
/// Consolidated here so the command modules that interpolate user-supplied ids
/// (church/service ids, etc.) into request URLs stop each carrying their own copy.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn url_encode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(url_encode("abcXYZ-09_.~"), "abcXYZ-09_.~");
        assert_eq!(url_encode("a b&c#d=e"), "a%20b%26c%23d%3De");
        assert_eq!(
            url_encode("550e8400-e29b-41d4-a716-446655440000"),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(url_encode(""), "");
    }

    #[test]
    fn lock_recover_returns_inner_after_poison() {
        // A poisoned mutex must still hand back its inner guard so one panicked
        // thread can't crash every later lock.
        let m = Arc::new(Mutex::new(1u8));
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert!(m.lock().is_err(), "precondition: the mutex is poisoned");
        *lock_recover(&m) = 42;
        assert_eq!(*lock_recover(&m), 42);
    }

    #[test]
    fn detect_platform_matches_the_build_target() {
        let p = detect_platform();
        if cfg!(target_os = "windows") {
            assert_eq!(p, Platform::Windows);
        } else if cfg!(target_os = "macos") {
            assert_eq!(p, Platform::MacOS);
        } else {
            assert_eq!(p, Platform::Linux);
        }
    }
}
