//! Small cross-cutting helpers shared across the shell modules.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use sundayrec_core::ffmpeg::Platform;

/// The bundle identifier Tauri names the app-data directory after. Read from
/// `tauri.conf.json` at COMPILE time (`include_str!`) so the two can never drift
/// — the same trick `path_guard`'s asset-scope tripwire uses.
fn bundle_identifier() -> Option<String> {
    let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).ok()?;
    conf.get("identifier")?.as_str().map(str::to_string)
}

/// The OS app-data directory — resolved WITHOUT a Tauri app handle.
///
/// `tauri::PathResolver::app_data_dir()` is `dirs::data_dir()?.join(identifier)`;
/// this mirrors that computation, because the two earliest observability seams
/// (the panic hook and the file-log writer) both have to be armed in `run()`,
/// before any `AppHandle` exists — and a crash between process start and
/// `setup()` is exactly the crash you most want a record of.
///
/// `setup` verifies the two agree ([`crate::crash::verify_dir_matches`]), so a
/// future change in Tauri's rule shows up as a warning rather than as records
/// quietly written somewhere nobody looks.
pub fn app_data_dir() -> Option<PathBuf> {
    let identifier = bundle_identifier()?;
    Some(platform_data_dir()?.join(identifier))
}

/// `dirs::data_dir()`, hand-rolled for the three desktop targets so no new
/// dependency is pulled in for two env-var lookups.
fn platform_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    }
}

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
    fn the_bundle_identifier_is_read_from_the_real_tauri_conf() {
        // If the identifier ever moves or is renamed, the crash ring + file log
        // would silently start writing to `<data>/` instead of
        // `<data>/no.sundayrec.app/` — beside every OTHER app's data.
        let id = bundle_identifier().expect("tauri.conf.json must carry an identifier");
        assert!(id.contains('.'), "a bundle identifier is reverse-DNS: {id}");
        assert!(!id.trim().is_empty());
    }

    #[test]
    fn the_app_data_dir_is_the_platform_dir_joined_with_the_identifier() {
        // The invariant Tauri's own resolver holds
        // (`dirs::data_dir()?.join(identifier)`), asserted against ours.
        let Some(dir) = app_data_dir() else {
            return; // no HOME/APPDATA in this environment — nothing to compare
        };
        let id = bundle_identifier().unwrap();
        assert_eq!(dir.file_name().unwrap().to_string_lossy(), id);
        assert_eq!(dir.parent().unwrap(), platform_data_dir().unwrap());
        assert!(dir.is_absolute(), "{}", dir.display());
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
