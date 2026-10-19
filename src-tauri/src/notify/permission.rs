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
//! - **macOS:** `UNUserNotificationCenter.getNotificationSettings`, and only
//!   when the process runs as a `.app` bundle (D2). Outside a bundle the call
//!   can abort the process, so the bundle is checked FIRST and the answer is
//!   `unknown`, as before. The call is asynchronous (an Objective-C
//!   completion block); it is bounded by [`MAC_SETTINGS_TIMEOUT`] so a call
//!   that never answers also ends as `unknown`, never as a hung page. «Not yet
//!   asked» (`notDetermined`) is `unknown` too: the plugin shows notifications
//!   through the legacy path and never asks. `denied`, or banners switched off
//!   (`alertSetting` disabled, i.e. style «Ingen»), is `denied`; the other
//!   authorised states are `granted`. Whether the legacy path obeys the same
//!   switches is the owner's trial (`docs/RIG-DAY.md`).
//! - **Asking (D2b):** the app never asks at start-up. The first «Send
//!   testvarsel» does, through [`request_and_current`]: only when the status
//!   is `notDetermined` (macOS never shows the dialog twice, so `granted` and
//!   `denied` are left alone), only inside a `.app`, with
//!   `requestAuthorizationWithOptions` (alert + sound). The dialog stays up
//!   until the user answers, so that wait has its own, roomy limit
//!   ([`MAC_REQUEST_TIMEOUT`]) and runs on the blocking pool; the status is
//!   then read again.
//! - **Everything else:** `unknown`.

use serde::{Deserialize, Serialize};
use std::sync::mpsc;
use std::time::Duration;
use ts_rs::TS;

/// How long the macOS settings call may take before the answer is `unknown`.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub const MAC_SETTINGS_TIMEOUT: Duration = Duration::from_millis(1500);

/// How long the macOS permission DIALOG may stay unanswered before we stop
/// waiting (and read the status as it then is). A person has to read and click.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub const MAC_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

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

/// `UNAuthorizationStatus`, reduced to what the decision needs.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacAuth {
    NotDetermined,
    Denied,
    /// Authorized, provisional or ephemeral.
    Allowed,
}

/// `UNNotificationSetting` for the alert (banner) style.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacAlert {
    NotSupported,
    Disabled,
    Enabled,
}

/// What `getNotificationSettings` reported.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacSettings {
    pub auth: MacAuth,
    pub alert: MacAlert,
}

/// The decision, pure.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn from_mac_settings(s: MacSettings) -> NotificationPermission {
    match s.auth {
        MacAuth::NotDetermined => NotificationPermission::Unknown,
        MacAuth::Denied => NotificationPermission::Denied,
        MacAuth::Allowed if s.alert == MacAlert::Disabled => NotificationPermission::Denied,
        MacAuth::Allowed => NotificationPermission::Granted,
    }
}

/// Is this process an `.app` bundle (path ends in `.app`, with an identifier)?
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn is_app_bundle(bundle_path: &str, identifier: Option<&str>) -> bool {
    bundle_path.trim_end_matches('/').ends_with(".app") && identifier.is_some_and(|i| !i.is_empty())
}

/// Run `start`, which must hand its answer to the sender (possibly from another
/// thread, later), and wait at most `timeout` for it.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn bounded<T: Send + 'static>(timeout: Duration, start: impl FnOnce(mpsc::Sender<T>)) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    start(tx);
    rx.recv_timeout(timeout).ok()
}

/// The macOS answer. `ask` is NOT called unless `in_bundle` — that is the
/// guard against the crash outside a bundle, and it is tested.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn resolve_mac(
    in_bundle: bool,
    timeout: Duration,
    ask: impl FnOnce(mpsc::Sender<MacSettings>),
) -> NotificationPermission {
    if !in_bundle {
        return NotificationPermission::Unknown;
    }
    bounded(timeout, ask).map_or(NotificationPermission::Unknown, from_mac_settings)
}

/// Should the app put the macOS permission dialog up? Only for a status that
/// was never decided: macOS does not show the dialog again after an answer, and
/// asking anyway would only add a pointless wait.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn should_ask(s: MacSettings) -> bool {
    s.auth == MacAuth::NotDetermined
}

/// Read the status; if it was never decided, ask once (`request` must hand
/// `()` to the sender when the user has answered), then read it again. `read`
/// and `request` are NOT called unless `in_bundle`.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn resolve_mac_asking(
    in_bundle: bool,
    read_timeout: Duration,
    request_timeout: Duration,
    mut read: impl FnMut(mpsc::Sender<MacSettings>),
    request: impl FnOnce(mpsc::Sender<()>),
) -> NotificationPermission {
    if !in_bundle {
        return NotificationPermission::Unknown;
    }
    let Some(first) = bounded(read_timeout, &mut read) else {
        return NotificationPermission::Unknown;
    };
    if !should_ask(first) {
        return from_mac_settings(first);
    }
    // The answer (or the lack of one) is read from the system, not from the
    // completion handler's `granted` flag: the banner style matters too.
    let _ = bounded(request_timeout, request);
    bounded(read_timeout, &mut read).map_or(NotificationPermission::Unknown, from_mac_settings)
}

/// «Send testvarsel» on a machine that may never have been asked: put the macOS
/// dialog up if (and only if) the status is undecided, then return the status.
/// Blocks until the user answers or [`MAC_REQUEST_TIMEOUT`]; call from the
/// blocking pool. Other platforms: same as [`current`].
pub fn request_and_current(identifier: &str) -> NotificationPermission {
    #[cfg(target_os = "macos")]
    {
        let _ = identifier;
        resolve_mac_asking(
            mac_un::in_app_bundle(),
            MAC_SETTINGS_TIMEOUT,
            MAC_REQUEST_TIMEOUT,
            mac_un::request_settings,
            mac_un::request_authorization,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        current(identifier)
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
    #[cfg(target_os = "macos")]
    {
        let _ = identifier;
        resolve_mac(
            mac_un::in_app_bundle(),
            MAC_SETTINGS_TIMEOUT,
            mac_un::request_settings,
        )
    }
    #[cfg(not(any(windows, target_os = "macos")))]
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

#[cfg(target_os = "macos")]
mod mac_un {
    use super::{is_app_bundle, MacAlert, MacAuth, MacSettings};
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSBundle, NSError};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNNotificationSetting,
        UNNotificationSettings, UNUserNotificationCenter,
    };
    use std::panic::AssertUnwindSafe;
    use std::ptr::NonNull;
    use std::sync::mpsc::Sender;

    pub fn in_app_bundle() -> bool {
        let bundle = NSBundle::mainBundle();
        let path = bundle.bundlePath().to_string();
        let id = bundle.bundleIdentifier().map(|i| i.to_string());
        is_app_bundle(&path, id.as_deref())
    }

    /// Put the system permission dialog up (alert + sound); the completion
    /// block sends `()` once the user has answered. Only call when
    /// [`in_app_bundle`], and only when the status is `notDetermined`.
    pub fn request_authorization(tx: Sender<()>) {
        let block = RcBlock::new(move |_granted: Bool, _error: *mut NSError| {
            let _ = tx.send(());
        });
        let _ = objc2::exception::catch(AssertUnwindSafe(|| {
            UNUserNotificationCenter::currentNotificationCenter()
                .requestAuthorizationWithOptions_completionHandler(
                    UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                    &block,
                );
        }));
    }

    /// Fire `getNotificationSettings`; the completion block sends the result.
    /// Only call when [`in_app_bundle`] — `UNUserNotificationCenter` aborts the
    /// process outside a bundle.
    pub fn request_settings(tx: Sender<MacSettings>) {
        let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            // SAFETY: UserNotifications passes a valid settings object for the
            // duration of the callback.
            let s = unsafe { settings.as_ref() };
            let auth = match s.authorizationStatus() {
                x if x == UNAuthorizationStatus::NotDetermined => MacAuth::NotDetermined,
                x if x == UNAuthorizationStatus::Denied => MacAuth::Denied,
                _ => MacAuth::Allowed,
            };
            let alert = match s.alertSetting() {
                x if x == UNNotificationSetting::Disabled => MacAlert::Disabled,
                x if x == UNNotificationSetting::Enabled => MacAlert::Enabled,
                _ => MacAlert::NotSupported,
            };
            let _ = tx.send(MacSettings { auth, alert });
        });
        // An Objective-C exception must never unwind into Rust; if one is
        // thrown, no answer is sent and the caller's timeout makes it `unknown`.
        let _ = objc2::exception::catch(AssertUnwindSafe(|| {
            UNUserNotificationCenter::currentNotificationCenter()
                .getNotificationSettingsWithCompletionHandler(&block);
        }));
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

    const T: Duration = Duration::from_millis(100);

    fn settings(auth: MacAuth, alert: MacAlert) -> MacSettings {
        MacSettings { auth, alert }
    }

    #[test]
    fn mac_decision_reads_the_authorisation_and_the_banner_style() {
        use MacAlert::*;
        use MacAuth::*;
        use NotificationPermission as P;
        assert_eq!(from_mac_settings(settings(Allowed, Enabled)), P::Granted);
        assert_eq!(
            from_mac_settings(settings(Allowed, NotSupported)),
            P::Granted
        );
        assert_eq!(from_mac_settings(settings(Allowed, Disabled)), P::Denied);
        assert_eq!(from_mac_settings(settings(Denied, Enabled)), P::Denied);
        assert_eq!(
            from_mac_settings(settings(NotDetermined, Enabled)),
            P::Unknown
        );
    }

    #[test]
    fn only_an_app_bundle_with_an_identifier_counts() {
        assert!(is_app_bundle("/Applications/SundayRec.app", Some("app.x")));
        assert!(is_app_bundle("/Applications/SundayRec.app/", Some("app.x")));
        assert!(!is_app_bundle("/repo/target/debug", None));
        assert!(!is_app_bundle("/repo/target/debug", Some("app.x")));
        assert!(!is_app_bundle("/Applications/SundayRec.app", None));
        assert!(!is_app_bundle("/Applications/SundayRec.app", Some("")));
    }

    #[test]
    fn outside_a_bundle_the_os_is_never_asked_and_the_answer_is_unknown() {
        let r = resolve_mac(false, T, |_| {
            panic!("UNUserNotificationCenter must not be touched outside a bundle")
        });
        assert_eq!(r, NotificationPermission::Unknown);
    }

    #[test]
    fn inside_a_bundle_the_answer_comes_from_the_completion_handler() {
        let r = resolve_mac(true, T, |tx| {
            // Answers from another thread, like the real completion block.
            std::thread::spawn(move || {
                tx.send(settings(MacAuth::Denied, MacAlert::Enabled)).ok();
            });
        });
        assert_eq!(r, NotificationPermission::Denied);
    }

    #[test]
    fn a_call_that_never_answers_ends_as_unknown_within_the_limit() {
        let t0 = std::time::Instant::now();
        // The stand-in keeps the sender alive and never sends.
        let (keep, _rx) = mpsc::channel::<mpsc::Sender<MacSettings>>();
        let r = resolve_mac(true, T, |tx| {
            keep.send(tx).ok();
        });
        assert_eq!(r, NotificationPermission::Unknown);
        assert!(t0.elapsed() < Duration::from_secs(2), "bounded in time");
    }

    /// The real thing, in a real (unbundled) test process: must neither crash
    /// nor claim to know.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_real_macos_call_in_a_test_binary_is_unknown_and_does_not_crash() {
        assert!(!mac_un::in_app_bundle());
        assert_eq!(current("app.test"), NotificationPermission::Unknown);
    }

    #[test]
    fn we_ask_only_when_the_status_was_never_decided() {
        use MacAlert::*;
        use MacAuth::*;
        // (status, asks?) — macOS shows the dialog once; granted/denied stay.
        for (auth, asks) in [(NotDetermined, true), (Denied, false), (Allowed, false)] {
            assert_eq!(should_ask(settings(auth, Enabled)), asks, "{auth:?}");
        }
    }

    /// A stand-in system: a status that changes when the dialog is answered.
    struct Fake {
        before: MacSettings,
        after: MacSettings,
        reads: std::sync::atomic::AtomicUsize,
        asks: std::sync::atomic::AtomicUsize,
    }

    impl Fake {
        fn new(before: MacSettings, after: MacSettings) -> Self {
            Fake {
                before,
                after,
                reads: Default::default(),
                asks: Default::default(),
            }
        }
        fn run(&self) -> NotificationPermission {
            use std::sync::atomic::Ordering::SeqCst;
            resolve_mac_asking(
                true,
                T,
                T,
                |tx| {
                    self.reads.fetch_add(1, SeqCst);
                    // Once asked, the system reports the new status.
                    let s = if self.asks.load(SeqCst) > 0 {
                        self.after
                    } else {
                        self.before
                    };
                    tx.send(s).ok();
                },
                |tx| {
                    self.asks.fetch_add(1, SeqCst);
                    tx.send(()).ok();
                },
            )
        }
        fn counts(&self) -> (usize, usize) {
            use std::sync::atomic::Ordering::SeqCst;
            (self.reads.load(SeqCst), self.asks.load(SeqCst))
        }
    }

    #[test]
    fn outside_a_bundle_neither_the_status_nor_the_dialog_is_touched() {
        let r = resolve_mac_asking(
            false,
            T,
            T,
            |_| panic!("must not read outside a bundle"),
            |_| panic!("must not ask outside a bundle"),
        );
        assert_eq!(r, NotificationPermission::Unknown);
    }

    #[test]
    fn an_undecided_status_asks_once_and_reports_the_answer() {
        for (after, want) in [
            (
                settings(MacAuth::Allowed, MacAlert::Enabled),
                NotificationPermission::Granted,
            ),
            (
                settings(MacAuth::Denied, MacAlert::Enabled),
                NotificationPermission::Denied,
            ),
        ] {
            let sys = Fake::new(settings(MacAuth::NotDetermined, MacAlert::Enabled), after);
            assert_eq!(sys.run(), want);
            // Asked once; the status is read again afterwards.
            assert_eq!(sys.counts(), (2, 1));
        }
    }

    #[test]
    fn a_decided_status_never_asks_again() {
        for (auth, want) in [
            (MacAuth::Allowed, NotificationPermission::Granted),
            (MacAuth::Denied, NotificationPermission::Denied),
        ] {
            let s = settings(auth, MacAlert::Enabled);
            let sys = Fake::new(s, s);
            assert_eq!(sys.run(), want);
            assert_eq!(sys.counts(), (1, 0), "{auth:?}: no new request");
        }
    }

    #[test]
    fn a_dialog_nobody_answers_ends_undecided_within_the_limit() {
        let t0 = std::time::Instant::now();
        let undecided = settings(MacAuth::NotDetermined, MacAlert::Enabled);
        let (keep, _rx) = mpsc::channel::<mpsc::Sender<()>>();
        let r = resolve_mac_asking(
            true,
            T,
            T,
            move |tx| {
                tx.send(undecided).ok();
            },
            |tx| {
                keep.send(tx).ok();
            },
        );
        assert_eq!(r, NotificationPermission::Unknown);
        assert!(t0.elapsed() < Duration::from_secs(2), "bounded in time");
    }

    #[test]
    fn a_status_that_never_answers_does_not_ask() {
        let (keep, _rx) = mpsc::channel::<mpsc::Sender<MacSettings>>();
        let r = resolve_mac_asking(
            true,
            T,
            T,
            |tx| {
                keep.send(tx).ok();
            },
            |_| panic!("must not ask when the status could not be read"),
        );
        assert_eq!(r, NotificationPermission::Unknown);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_real_request_in_a_test_binary_does_not_ask_and_does_not_crash() {
        assert_eq!(
            request_and_current("app.test"),
            NotificationPermission::Unknown
        );
    }

    #[test]
    fn a_dropped_handler_is_unknown_not_a_hang() {
        let r = resolve_mac(true, Duration::from_secs(5), drop);
        assert_eq!(r, NotificationPermission::Unknown);
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
