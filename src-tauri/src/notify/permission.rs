//! Does the OS actually SHOW SundayRec's notifications?
//!
//! Every failure alert in the app is a native notification (see `notify`), so a
//! machine where notifications are switched off for SundayRec is a machine
//! where nobody hears that Sunday's recording failed. This module answers that
//! question as honestly as each platform lets it.
//!
//! ## Why not `tauri-plugin-notification`'s own permission API
//!
//! On desktop, `permission_state()` and `request_permission()` always answer
//! `Granted` — the plugin does not ask the OS at all. Using them would turn the
//! «Hvem får beskjed?» card green on a machine that shows nothing.
//!
//! ## Per platform
//!
//! - **Windows:** two registry values under `HKEY_CURRENT_USER` decide it — the
//!   global switch (`…\PushNotifications\ToastEnabled`) and the per-app switch
//!   (`…\Notifications\Settings\<AUMID>\Enabled`, where the AUMID is the bundle
//!   identifier the installer registers). Either one at `0` means off; a value
//!   that is absent means the Windows default, which is on.
//! - **macOS:** `unknown`. The real answer lives in `UNUserNotificationCenter`,
//!   whose settings call takes an Objective-C block and crashes outside an
//!   `.app` bundle, and whether it even reflects the legacy
//!   `NSUserNotification` path the plugin shows notifications through has not
//!   been proven on a real Mac. Until it has, the page offers «Send
//!   testvarsel» and a button to the Notifications settings instead of a
//!   green light it cannot stand behind (`docs/VARSLING.md`).
//! - **Everything else:** `unknown`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What the OS says about showing SundayRec's notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "NotificationPermission.ts")]
#[serde(rename_all = "camelCase")]
pub enum NotificationPermission {
    /// The OS will show them.
    Granted,
    /// Switched off — globally, or for SundayRec.
    Denied,
    /// This platform does not let us find out (see the module docs).
    Unknown,
}

/// The two Windows switches, as read from the registry. `None` = the value is
/// absent, which Windows treats as on.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn from_windows_switches(global: Option<u32>, per_app: Option<u32>) -> NotificationPermission {
    if global == Some(0) || per_app == Some(0) {
        NotificationPermission::Denied
    } else {
        NotificationPermission::Granted
    }
}

/// Ask the OS. `identifier` is the bundle identifier (`tauri.conf.json`), which
/// is the AUMID the Windows installer registers notifications under.
pub fn current(identifier: &str) -> NotificationPermission {
    #[cfg(windows)]
    {
        from_windows_switches(
            windows_reg::read_hkcu_dword(
                r"Software\Microsoft\Windows\CurrentVersion\PushNotifications",
                "ToastEnabled",
            ),
            windows_reg::read_hkcu_dword(
                &format!(
                    r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings\{identifier}"
                ),
                "Enabled",
            ),
        )
    }
    #[cfg(not(windows))]
    {
        let _ = identifier;
        NotificationPermission::Unknown
    }
}

/// The OS settings page where notifications for apps are switched on, if this
/// platform has one we can open.
pub fn settings_url() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("x-apple.systempreferences:com.apple.Notifications-Settings.extension")
    } else if cfg!(windows) {
        Some("ms-settings:notifications")
    } else {
        None
    }
}

#[cfg(windows)]
mod windows_reg {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// One DWORD under `HKEY_CURRENT_USER`, or `None` when it is absent or
    /// unreadable.
    pub fn read_hkcu_dword(subkey: &str, value: &str) -> Option<u32> {
        let subkey = wide(subkey);
        let value = wide(value);
        let mut data: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: both strings are NUL-terminated UTF-16 buffers that outlive
        // the call; `data`/`size` are a valid out-pointer pair for a DWORD.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut data as *mut u32).cast(),
                &mut size,
            )
        };
        (status == ERROR_SUCCESS).then_some(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_is_on_unless_a_switch_says_off() {
        assert_eq!(
            from_windows_switches(None, None),
            NotificationPermission::Granted
        );
        assert_eq!(
            from_windows_switches(Some(1), Some(1)),
            NotificationPermission::Granted
        );
        assert_eq!(
            from_windows_switches(Some(0), None),
            NotificationPermission::Denied,
            "notifications off for the whole machine"
        );
        assert_eq!(
            from_windows_switches(None, Some(0)),
            NotificationPermission::Denied,
            "notifications off for SundayRec"
        );
    }

    #[test]
    fn the_permission_serialises_to_what_the_renderer_matches_on() {
        for (p, wire) in [
            (NotificationPermission::Granted, "\"granted\""),
            (NotificationPermission::Denied, "\"denied\""),
            (NotificationPermission::Unknown, "\"unknown\""),
        ] {
            assert_eq!(serde_json::to_string(&p).unwrap(), wire);
        }
    }
}
