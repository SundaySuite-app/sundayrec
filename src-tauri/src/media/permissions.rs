//! macOS camera / microphone authorization (TCC).
//!
//! The biggest robustness gap vs the old Electron build: it called
//! `systemPreferences.getMediaAccessStatus` / `askForMediaAccess`, so a denied
//! camera produced a CLEAR "open System Settings" message. ffmpeg's avfoundation,
//! by contrast, often emits only "Input/output error" or simply zero frames on a
//! denied device — which surfaced to the user as the misleading
//! "Fant ikke kameraet" (camera not found) even though the camera was right there.
//!
//! Here we ask AVFoundation directly for the authorization status BEFORE we spend
//! ~10 s walking the capture-mode matrix, so the preview and a video recording can
//! fail fast with an actionable message when access is denied or restricted.
//!
//! **Crash-safety.** The macOS path looks the class up with `AnyClass::get` and
//! returns [`AuthStatus::Unknown`] if anything is missing (framework not loaded,
//! class absent, unexpected status integer). A determination we *can't* make
//! therefore degrades to exactly today's behaviour — proceed and let ffmpeg try —
//! never a panic at the worst possible moment. Non-macOS is always
//! [`AuthStatus::Authorized`] (Windows/Linux gate camera access differently and
//! ffmpeg's own error is the source of truth there).
//!
//! ⚠️ HARDWARE-UNVERIFIED on the objc path (compiles + wired against the real
//! AVFoundation API; the actual status integers need a real Mac to confirm).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Which device's access we're asking about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Camera,
    Microphone,
}

impl MediaKind {
    /// The avfoundation media-type string — `AVMediaTypeVideo` / `AVMediaTypeAudio`.
    /// These are stable Apple constants: video = `"vide"`, audio = `"soun"`.
    /// Passing the raw string is equivalent to passing the framework constant,
    /// and avoids linking an extern `static` just to read two known values.
    #[cfg(target_os = "macos")]
    fn media_type(self) -> &'static str {
        match self {
            MediaKind::Camera => "vide",
            MediaKind::Microphone => "soun",
        }
    }
}

/// Authorization status, mirroring Apple's `AVAuthorizationStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "AuthStatus.ts")]
#[serde(rename_all = "camelCase")]
pub enum AuthStatus {
    /// Never asked — opening the device will trigger the system prompt.
    NotDetermined,
    /// Parental controls / MDM forbid it; the user cannot grant it themselves.
    Restricted,
    /// Explicitly denied; only System Settings → Privacy can re-enable it.
    Denied,
    /// Granted — go ahead.
    Authorized,
    /// Could not be determined (non-macOS, or the AVFoundation lookup failed).
    /// Treated as "proceed" so we never regress below today's behaviour.
    Unknown,
}

impl AuthStatus {
    /// True when access is positively blocked and opening the device is pointless
    /// — the caller should surface an actionable message instead of spawning
    /// ffmpeg and walking the mode matrix for ~10 s. `NotDetermined` is *not*
    /// blocked: opening the device is exactly what triggers the OS prompt, so we
    /// let it through.
    pub fn is_blocked(self) -> bool {
        matches!(self, AuthStatus::Denied | AuthStatus::Restricted)
    }

    /// Map the raw `AVAuthorizationStatus` integer (0..=3) to the enum. Only the
    /// macOS `status()` path calls this, but the pure mapping is unit-tested on
    /// every platform — so allow it to be "unused" off macOS rather than gating
    /// the function (and its test) behind `cfg(macos)`.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn from_raw(raw: isize) -> Self {
        match raw {
            0 => AuthStatus::NotDetermined,
            1 => AuthStatus::Restricted,
            2 => AuthStatus::Denied,
            3 => AuthStatus::Authorized,
            _ => AuthStatus::Unknown,
        }
    }
}

/// The current camera + microphone authorization, for the UI / preflight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "MediaPermissions.ts")]
#[serde(rename_all = "camelCase")]
pub struct MediaPermissions {
    pub camera: AuthStatus,
    pub microphone: AuthStatus,
}

/// Query the authorization status for one device kind.
#[cfg(target_os = "macos")]
pub fn status(kind: MediaKind) -> AuthStatus {
    use objc2::msg_send;
    use objc2::runtime::AnyClass;
    use objc2_foundation::NSString;

    // Crash-safe lookup: if AVFoundation isn't loaded or the class is absent we
    // get `None` → Unknown → caller proceeds exactly as before.
    let Some(cls) = AnyClass::get(c"AVCaptureDevice") else {
        return AuthStatus::Unknown;
    };
    let media = NSString::from_str(kind.media_type());
    // +[AVCaptureDevice authorizationStatusForMediaType:] returns an NSInteger.
    // SAFETY: a documented class method taking an NSString and returning an
    // integer; `cls` is a valid class and `media` a live NSString for the call.
    let raw: isize = unsafe { msg_send![cls, authorizationStatusForMediaType: &*media] };
    AuthStatus::from_raw(raw)
}

#[cfg(not(target_os = "macos"))]
pub fn status(_kind: MediaKind) -> AuthStatus {
    // No TCC on Windows/Linux — ffmpeg's own open error is the source of truth.
    AuthStatus::Authorized
}

/// Snapshot both camera and microphone authorization.
pub fn current() -> MediaPermissions {
    MediaPermissions {
        camera: status(MediaKind::Camera),
        microphone: status(MediaKind::Microphone),
    }
}

/// The message for a BLOCKED device — leading with the stable code the shell
/// localises on. `None` when access isn't blocked.
///
/// ## F2-I18N-R2: the code leads, the prose is a reserve
///
/// This used to be a ready-to-show NORWEGIAN sentence, and it became the whole
/// body of an `AppError::Recording` — so a volunteer whose app is in Polish got
/// Norwegian in the one message that says why recording refused to start. The
/// shell resolves the code out of the error text
/// (`nativeErrorSuffixFromText`), exactly the way `no_save_folder` works in
/// `settings.rs`; the English half is what a log, or a shell that predates the
/// code, shows.
///
/// The two devices get two codes on purpose. `errorPermission` is the
/// microphone's sentence — "Microphone access denied" for a blocked CAMERA is a
/// true-sounding sentence about the wrong device, and the volunteer would go
/// and open the wrong pane.
pub fn blocked_message(kind: MediaKind, s: AuthStatus) -> Option<String> {
    if !s.is_blocked() {
        return None;
    }
    let (code, device, pane) = match kind {
        MediaKind::Camera => ("camera_permission_denied", "the camera", "Camera"),
        MediaKind::Microphone => ("device_permission_denied", "the microphone", "Microphone"),
    };
    let why = match s {
        AuthStatus::Restricted => {
            "Access is blocked by a system administrator (parental controls/MDM)."
        }
        _ => "The app does not have access.",
    };
    Some(format!(
        "{code}: cannot use {device}. {why} Open System Settings → Privacy & \
         Security → {pane}, switch SundayRec on, and try again."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_raw_maps_avauthorizationstatus() {
        assert_eq!(AuthStatus::from_raw(0), AuthStatus::NotDetermined);
        assert_eq!(AuthStatus::from_raw(1), AuthStatus::Restricted);
        assert_eq!(AuthStatus::from_raw(2), AuthStatus::Denied);
        assert_eq!(AuthStatus::from_raw(3), AuthStatus::Authorized);
        // Anything Apple might add later degrades safely to Unknown (→ proceed).
        assert_eq!(AuthStatus::from_raw(99), AuthStatus::Unknown);
        assert_eq!(AuthStatus::from_raw(-1), AuthStatus::Unknown);
    }

    #[test]
    fn only_denied_and_restricted_block() {
        assert!(AuthStatus::Denied.is_blocked());
        assert!(AuthStatus::Restricted.is_blocked());
        // NotDetermined must NOT block — opening the device triggers the prompt.
        assert!(!AuthStatus::NotDetermined.is_blocked());
        assert!(!AuthStatus::Authorized.is_blocked());
        // Unknown must NOT block — never regress below today's "just try" path.
        assert!(!AuthStatus::Unknown.is_blocked());
    }

    #[test]
    fn blocked_message_only_for_blocked_and_names_the_pane() {
        // Authorized / not-determined / unknown → no message (proceed).
        assert!(blocked_message(MediaKind::Camera, AuthStatus::Authorized).is_none());
        assert!(blocked_message(MediaKind::Camera, AuthStatus::NotDetermined).is_none());
        assert!(blocked_message(MediaKind::Microphone, AuthStatus::Unknown).is_none());

        let cam = blocked_message(MediaKind::Camera, AuthStatus::Denied).unwrap();
        assert!(cam.contains("Camera"), "names the camera pane: {cam}");
        assert!(
            cam.contains("System Settings"),
            "points at System Settings: {cam}"
        );

        let mic = blocked_message(MediaKind::Microphone, AuthStatus::Restricted).unwrap();
        assert!(
            mic.contains("Microphone"),
            "names the microphone pane: {mic}"
        );
        assert!(
            mic.contains("administrator"),
            "restricted explains MDM/parental control: {mic}"
        );
    }

    /// F2-I18N-R2: the code LEADS, and the two devices do not share one.
    /// `nativeErrorSuffixFromText` scans the text for a known code, so a
    /// message that lost its prefix silently becomes `errorUnknown`.
    #[test]
    fn blocked_message_leads_with_its_own_code() {
        let cam = blocked_message(MediaKind::Camera, AuthStatus::Denied).unwrap();
        let mic = blocked_message(MediaKind::Microphone, AuthStatus::Denied).unwrap();
        assert!(cam.starts_with("camera_permission_denied: "), "{cam}");
        assert!(mic.starts_with("device_permission_denied: "), "{mic}");
        // English, both of them — a reserve in Norwegian is not a reserve.
        for m in [&cam, &mic] {
            assert!(!m.contains(['æ', 'ø', 'å']), "{m}");
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_is_always_authorized() {
        assert_eq!(status(MediaKind::Camera), AuthStatus::Authorized);
        assert_eq!(status(MediaKind::Microphone), AuthStatus::Authorized);
        assert!(!current().camera.is_blocked());
    }
}
