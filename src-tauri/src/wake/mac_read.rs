//! macOS READ side: what wakes the OS actually has scheduled, taken from IOKit
//! instead of from `pmset -g sched` text.
//!
//! ## Why not keep parsing `pmset`
//!
//! `pmset` is itself a thin shell over the IOKit power-management API. The
//! *write* side (`IOPMSchedulePowerEvent`) requires root, which is why scheduling
//! still goes through `pmset` behind an `osascript` admin prompt — bindings would
//! buy nothing there. But the *read* side, `IOPMCopyScheduledPowerEvents`, is
//! **unprivileged**. Calling it directly removes a process spawn, a locale- and
//! version-dependent output format, and a regex from the path that answers "is
//! Sunday's wake actually registered?".
//!
//! ## The caveat that does NOT go away
//!
//! Reading the source correctly is not the same as the source being right. On
//! Apple Silicon the power-management scheduler has documented oddities: cancels
//! of repeating events that do not take effect, and schedules that are active yet
//! absent from `pmset -g sched`. Whatever the discrepancy's origin, a perfect
//! reader of a lying source still reports a lie — so the verdict built on this
//! stays conservative: a wake we cannot see is reported as a MISMATCH (prompting
//! a re-register), never as "fine, it is probably there". And because IOKit and
//! `pmset` can disagree, [`super`] keeps the `pmset` read as a fallback rather
//! than deleting it: if IOKit reports nothing we still ask the text tool before
//! concluding nothing is scheduled.
//!
//! ## No new dependency
//!
//! This is ~20 lines of C FFI against two system frameworks that are already
//! linked into every macOS build. A CoreFoundation wrapper crate
//! (`objc2-core-foundation` is in the tree transitively) would add a manifest
//! dependency to save declaring six `extern "C"` functions — not a trade worth
//! making for a call site this small.

use chrono::NaiveDateTime;
use sundayrec_core::wake::VerifiedWake;

/// Seconds from the Unix epoch to the CoreFoundation epoch (2001-01-01 00:00 UTC).
/// `CFAbsoluteTime` counts from the latter.
pub const CF_EPOCH_UNIX_SECS: i64 = 978_307_200;

/// Convert a `CFAbsoluteTime` to the local wall clock the rest of the wake domain
/// works in.
///
/// `utc_offset_secs` is the caller's `Local` offset (east of UTC positive), passed
/// in rather than read here so this stays pure and testable.
pub fn cf_absolute_to_local(abs: f64, utc_offset_secs: i32) -> Option<NaiveDateTime> {
    if !abs.is_finite() {
        return None;
    }
    let unix = CF_EPOCH_UNIX_SECS as f64 + abs;
    // Guard the cast: an out-of-range float would saturate rather than error.
    if unix.abs() > 1e15 {
        return None;
    }
    let secs = unix.round() as i64 + utc_offset_secs as i64;
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.naive_utc())
}

/// The `kIOPMPowerEventTypeKey` values that mean "the machine comes up".
/// `sleep`/`shutdown` events live in the same list and must not be counted.
pub fn is_wake_event(event_type: &str) -> bool {
    matches!(
        event_type.to_ascii_lowercase().as_str(),
        "wake" | "poweron" | "wakepoweron"
    )
}

/// Read the OS's scheduled power events through IOKit, unprivileged.
///
/// Returns the wake-type events only. An empty vector means "IOKit reported no
/// wakes", which the caller treats as a reason to fall back to `pmset` rather
/// than as proof. `None` means this build has no IOKit at all (non-macOS).
///
/// ⚠️ HARDWARE-UNVERIFIED in one direction only: the FFI walk itself is exercised
/// by `live_iokit_read_is_callable_unprivileged` on any macOS host, but that the
/// events it returns correspond to wakes the machine will really perform needs a
/// sleep/wake cycle on a real box.
pub fn read_scheduled_wakes(utc_offset_secs: i32) -> Option<Vec<VerifiedWake>> {
    #[cfg(target_os = "macos")]
    {
        Some(imp::read(utc_offset_secs))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = utc_offset_secs;
        None
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{cf_absolute_to_local, is_wake_event};
    use std::ffi::{c_char, c_void, CString};
    use sundayrec_core::wake::VerifiedWake;

    type CFTypeRef = *const c_void;
    type CFIndex = isize;
    type CFTypeID = usize;

    /// `kCFStringEncodingUTF8`.
    const UTF8: u32 = 0x0800_0100;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        /// Returns a `CFArrayRef` of `CFDictionaryRef`, or NULL. Caller releases.
        /// Unprivileged — unlike `IOPMSchedulePowerEvent`, which needs root.
        fn IOPMCopyScheduledPowerEvents() -> CFTypeRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
        fn CFArrayGetTypeID() -> CFTypeID;
        fn CFArrayGetCount(a: CFTypeRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(a: CFTypeRef, i: CFIndex) -> CFTypeRef;
        fn CFDictionaryGetTypeID() -> CFTypeID;
        fn CFDictionaryGetValue(d: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFDateGetTypeID() -> CFTypeID;
        fn CFDateGetAbsoluteTime(d: CFTypeRef) -> f64;
        fn CFStringGetTypeID() -> CFTypeID;
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, enc: u32) -> CFTypeRef;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: CFIndex, enc: u32) -> u8;
    }

    /// A CFString we own for the duration of a lookup.
    struct OwnedKey(CFTypeRef);

    impl OwnedKey {
        fn new(name: &str) -> Option<Self> {
            let c = CString::new(name).ok()?;
            // SAFETY: `c` is a valid NUL-terminated UTF-8 buffer that outlives
            // the call; a NULL return is checked.
            let s = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) };
            if s.is_null() {
                None
            } else {
                Some(Self(s))
            }
        }
    }

    impl Drop for OwnedKey {
        fn drop(&mut self) {
            // SAFETY: created by `CFStringCreateWithCString` (a Create rule
            // reference we own) and released exactly once.
            unsafe { CFRelease(self.0) };
        }
    }

    /// Copy a `CFStringRef` out as a Rust `String`, or `None` if it is not a
    /// string / does not fit.
    fn cf_string(value: CFTypeRef) -> Option<String> {
        if value.is_null() {
            return None;
        }
        // SAFETY: `value` is non-null and came from a CF container, so asking for
        // its type id is defined.
        if unsafe { CFGetTypeID(value) } != unsafe { CFStringGetTypeID() } {
            return None;
        }
        let mut buf = vec![0i8; 512];
        // SAFETY: `buf` is a live, correctly-sized buffer; CF NUL-terminates.
        let ok = unsafe {
            CFStringGetCString(
                value,
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as CFIndex,
                UTF8,
            )
        };
        if ok == 0 {
            return None;
        }
        let bytes: Vec<u8> = buf
            .iter()
            .take_while(|b| **b != 0)
            .map(|b| *b as u8)
            .collect();
        String::from_utf8(bytes).ok()
    }

    /// The whole IOKit walk, with every container type checked before it is used.
    pub fn read(utc_offset_secs: i32) -> Vec<VerifiedWake> {
        let (Some(time_key), Some(app_key), Some(type_key)) = (
            OwnedKey::new("time"),        // kIOPMPowerEventTimeKey
            OwnedKey::new("scheduledby"), // kIOPMPowerEventAppNameKey
            OwnedKey::new("eventtype"),   // kIOPMPowerEventTypeKey
        ) else {
            return Vec::new();
        };

        // SAFETY: a Copy-rule reference we own and release below. NULL is the
        // documented "nothing scheduled / not available" answer.
        let array = unsafe { IOPMCopyScheduledPowerEvents() };
        if array.is_null() {
            return Vec::new();
        }
        // A defensive type check: if this is somehow not an array, release and
        // report nothing rather than indexing into it.
        if unsafe { CFGetTypeID(array) } != unsafe { CFArrayGetTypeID() } {
            unsafe { CFRelease(array) };
            return Vec::new();
        }

        let mut out = Vec::new();
        // SAFETY: `array` is a verified CFArray.
        let count = unsafe { CFArrayGetCount(array) };
        for i in 0..count {
            // SAFETY: `i` is in range; the returned reference is borrowed from
            // the array (Get rule) and must NOT be released.
            let item = unsafe { CFArrayGetValueAtIndex(array, i) };
            if item.is_null() || unsafe { CFGetTypeID(item) } != unsafe { CFDictionaryGetTypeID() }
            {
                continue;
            }

            // SAFETY: `item` is a verified CFDictionary; keys are live CFStrings.
            let type_val = unsafe { CFDictionaryGetValue(item, type_key.0) };
            let event_type = cf_string(type_val).unwrap_or_default();
            if !is_wake_event(&event_type) {
                continue;
            }

            let time_val = unsafe { CFDictionaryGetValue(item, time_key.0) };
            if time_val.is_null()
                || unsafe { CFGetTypeID(time_val) } != unsafe { CFDateGetTypeID() }
            {
                continue;
            }
            let abs = unsafe { CFDateGetAbsoluteTime(time_val) };
            let Some(when) = cf_absolute_to_local(abs, utc_offset_secs) else {
                continue;
            };

            let owner = cf_string(unsafe { CFDictionaryGetValue(item, app_key.0) })
                .unwrap_or_else(|| "unknown".to_string());
            out.push(VerifiedWake {
                scheduled_at: when,
                owner_label: owner,
            });
        }

        // SAFETY: the Copy-rule reference from IOPMCopyScheduledPowerEvents,
        // released exactly once now that nothing borrowed from it is still held
        // (owner/type strings were copied into owned Rust `String`s).
        unsafe { CFRelease(array) };
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cf_absolute_time_maps_to_the_local_wall_clock() {
        // CFAbsoluteTime 0 is 2001-01-01 00:00:00 UTC by definition.
        assert_eq!(
            cf_absolute_to_local(0.0, 0).unwrap(),
            NaiveDateTime::parse_from_str("2001-01-01 00:00:00", "%Y-%m-%d %H:%M:%S").unwrap()
        );
        // A +02:00 summer offset moves the wall clock forward, not the instant.
        assert_eq!(
            cf_absolute_to_local(0.0, 7_200).unwrap(),
            NaiveDateTime::parse_from_str("2001-01-01 02:00:00", "%Y-%m-%d %H:%M:%S").unwrap()
        );
        // Sub-second precision rounds to the nearest second — pmset schedules on
        // whole minutes, so the tolerance match never sees the difference.
        assert_eq!(
            cf_absolute_to_local(1.6, 0).unwrap(),
            NaiveDateTime::parse_from_str("2001-01-01 00:00:02", "%Y-%m-%d %H:%M:%S").unwrap()
        );
        // Garbage in is None, not a panic or a year-292277026596 timestamp.
        assert!(cf_absolute_to_local(f64::NAN, 0).is_none());
        assert!(cf_absolute_to_local(f64::INFINITY, 0).is_none());
        assert!(cf_absolute_to_local(1e18, 0).is_none());
    }

    #[test]
    fn only_wake_type_events_are_counted() {
        // The same list carries scheduled SLEEP and SHUTDOWN events; counting one
        // of those as a wake would make the verification panel claim Sunday is
        // covered by an event that puts the machine DOWN.
        assert!(is_wake_event("wake"));
        assert!(is_wake_event("poweron"));
        assert!(is_wake_event("wakepoweron"));
        assert!(is_wake_event("WAKE"));
        assert!(!is_wake_event("sleep"));
        assert!(!is_wake_event("shutdown"));
        assert!(!is_wake_event(""));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn live_iokit_read_is_callable_unprivileged() {
        // Runs for real in the gate on any Mac: the point is that the FFI walk
        // is memory-safe and needs no root. The host may legitimately have zero
        // scheduled events, so the LIST is not asserted — only that we get one.
        let wakes = read_scheduled_wakes(0);
        assert!(wakes.is_some(), "IOKit read must be available on macOS");
        for w in wakes.unwrap() {
            assert!(w.scheduled_at.and_utc().timestamp() > 0);
        }
    }
}
