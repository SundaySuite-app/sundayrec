//! What the recorder does when the live capture stream reports a fault.
//!
//! The audio library (cpal 0.18) reports everything that happens to a running
//! stream through ONE error callback, tagged with an `ErrorKind`. Some of those
//! reports mean the stream is dead (the device was unplugged, the sample rate
//! changed under it), some mean it is merely busy, and some are not errors at
//! all — a single buffer glitch, or a note that the OS rerouted the stream and
//! it carries on. Treating them alike was the A3a state: every report except
//! `Xrun`/`RealtimeDenied` ended the segment and reconnected.
//!
//! This module is the decision, as a pure table. The `src-tauri` shell maps
//! cpal's `ErrorKind` onto [`StreamFault`] (`native_capture::stream::
//! stream_fault`) and does what [`stream_fault_action`] says. The split keeps
//! cpal out of this crate and keeps the rule testable without a sound card.
//!
//! | Fault | Action | Why |
//! |---|---|---|
//! | [`StreamFault::DeviceNotAvailable`] | reconnect, `device_disconnected` | the device is gone; the back-off waits for it (and is cut short by the OS device-list signal) |
//! | [`StreamFault::StreamInvalidated`] | reconnect, `device_disconnected` | the stream must be rebuilt — on macOS this is a sample-rate change of the device; on WASAPI also a change of the Windows default input under a default-device stream (WASAPI never rebinds). The respawn reopens at the device's CURRENT rate ([`reconnect_keeps_rate_pin`]), and when that differs from the deliverable's, the supervisor starts a new deliverable instead of `-c copy`-joining two rates |
//! | [`StreamFault::DeviceBusy`] | retry, `device_busy` | the device is THERE but held (WASAPI `AUDCLNT_E_DEVICE_IN_USE`, ASIO `HardwareStuck`). Same recovery policy as a reconnect — a timed back-off — but the code says what actually happened, and no device-list change will cut the wait short |
//! | [`StreamFault::PermissionDenied`] | **fatal**, `device_permission_denied` | a retry cannot grant a permission; the session ends with the sentence that tells the volunteer what to do |
//! | [`StreamFault::DeviceChanged`] | keep recording | cpal: "the stream remains active and no rebuild is required" |
//! | [`StreamFault::Xrun`] | keep recording | one glitch; the native engine COUNTS it (A3c, `RecordingTelemetry::native_xruns`) |
//! | [`StreamFault::RealtimeDenied`] | keep recording | audio still flows, at a higher risk of glitches; logged, not counted — it is one report at start-up, and the glitches it makes likelier arrive as `Xrun` |
//! | [`StreamFault::Other`] | reconnect, `device_disconnected` | anything else (backend error, host gone, resource limit) keeps the pre-A3b behaviour: tear down and reconnect |
//!
//! "Reconnect" and "retry" deliberately share one mechanism — the
//! `RecordingSession::on_unexpected_exit` time budget and back-off ladder. What
//! differs is the code the recovery policy and the UI see. Only a code in
//! [`crate::recorder::is_fatal_reconnect_error`] ends the session, and the
//! table below is pinned against that function, so the two cannot drift.

use crate::errors::RecordingErrorCode;

/// A fault reported by a live capture stream, reduced to the classes the
/// recorder decides on. Mirrors cpal 0.18's `ErrorKind`; every kind not named
/// here is [`StreamFault::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamFault {
    /// The device disappeared (USB pulled, driver reset).
    DeviceNotAvailable,
    /// The stream's configuration is no longer valid and it must be rebuilt
    /// (macOS: the device's nominal sample rate changed).
    StreamInvalidated,
    /// The device exists but another client holds it.
    DeviceBusy,
    /// The OS refused access to the device.
    PermissionDenied,
    /// The OS rerouted the stream to another device; it keeps running.
    DeviceChanged,
    /// A buffer under-/overrun — a possible audible glitch, nothing more.
    Xrun,
    /// Real-time scheduling was refused for the audio thread.
    RealtimeDenied,
    /// Any other kind (backend error, host unavailable, resource limit, …).
    Other,
}

/// What the capture loop does with a [`StreamFault`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFaultAction {
    /// The stream is still alive: log it and keep recording.
    KeepRecording,
    /// End this segment (the fragment is finalized and valid) and let the
    /// recovery policy reconnect. The code is transient.
    Reconnect(RecordingErrorCode),
    /// End this segment and the session. The code is fatal.
    Fatal(RecordingErrorCode),
}

impl StreamFaultAction {
    /// The error code the recovery policy is handed, or `None` when the
    /// stream keeps running.
    pub fn code(self) -> Option<RecordingErrorCode> {
        match self {
            StreamFaultAction::KeepRecording => None,
            StreamFaultAction::Reconnect(code) | StreamFaultAction::Fatal(code) => Some(code),
        }
    }
}

/// The table in the module docs.
pub fn stream_fault_action(fault: StreamFault) -> StreamFaultAction {
    use StreamFault as F;
    use StreamFaultAction as A;
    match fault {
        F::DeviceChanged | F::Xrun | F::RealtimeDenied => A::KeepRecording,
        F::DeviceNotAvailable | F::StreamInvalidated | F::Other => {
            A::Reconnect(RecordingErrorCode::DeviceDisconnected)
        }
        F::DeviceBusy => A::Reconnect(RecordingErrorCode::DeviceBusy),
        F::PermissionDenied => A::Fatal(RecordingErrorCode::DevicePermissionDenied),
    }
}

/// Should the reconnect after a segment that died of `fault` reopen the device
/// at the current deliverable's pinned sample rate?
///
/// Yes, normally: every `_rN` fragment of one deliverable must share a rate so
/// they can be `-c copy`-joined, and a device that comes back after a USB pull
/// comes back at its old rate anyway. **No after [`StreamFault::StreamInvalidated`]:**
/// that is the device's format changing under the stream (on macOS, its nominal
/// sample rate). Asking for the pinned rate would make the audio library
/// re-clock the device straight back, fighting whoever changed it, and on a
/// shared interface the two can invalidate each other indefinitely. The
/// reconnect instead opens at the device's current rate; the supervisor's
/// rate-mismatch check then closes the deliverable and continues in a new one.
///
/// `None` (the segment died some other way, or on the ffmpeg path) keeps the pin.
pub fn reconnect_keeps_rate_pin(fault: Option<StreamFault>) -> bool {
    !matches!(fault, Some(StreamFault::StreamInvalidated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::is_fatal_reconnect_error;

    const ALL: [StreamFault; 8] = [
        StreamFault::DeviceNotAvailable,
        StreamFault::StreamInvalidated,
        StreamFault::DeviceBusy,
        StreamFault::PermissionDenied,
        StreamFault::DeviceChanged,
        StreamFault::Xrun,
        StreamFault::RealtimeDenied,
        StreamFault::Other,
    ];

    #[test]
    fn an_unplugged_device_reconnects() {
        assert_eq!(
            stream_fault_action(StreamFault::DeviceNotAvailable),
            StreamFaultAction::Reconnect(RecordingErrorCode::DeviceDisconnected)
        );
    }

    #[test]
    fn a_sample_rate_change_reconnects() {
        // macOS reports a nominal-rate change of the device as
        // StreamInvalidated: the stream is rebuilt through the same reconnect.
        assert_eq!(
            stream_fault_action(StreamFault::StreamInvalidated),
            StreamFaultAction::Reconnect(RecordingErrorCode::DeviceDisconnected)
        );
    }

    #[test]
    fn a_busy_device_is_retried_under_its_own_code() {
        assert_eq!(
            stream_fault_action(StreamFault::DeviceBusy),
            StreamFaultAction::Reconnect(RecordingErrorCode::DeviceBusy)
        );
    }

    #[test]
    fn a_denied_permission_is_fatal_with_the_permission_code() {
        assert_eq!(
            stream_fault_action(StreamFault::PermissionDenied),
            StreamFaultAction::Fatal(RecordingErrorCode::DevicePermissionDenied)
        );
    }

    #[test]
    fn advisories_keep_the_stream_recording() {
        for f in [
            StreamFault::DeviceChanged,
            StreamFault::Xrun,
            StreamFault::RealtimeDenied,
        ] {
            assert_eq!(
                stream_fault_action(f),
                StreamFaultAction::KeepRecording,
                "{f:?}"
            );
            assert_eq!(stream_fault_action(f).code(), None);
        }
    }

    #[test]
    fn unclassified_faults_keep_the_old_reconnect() {
        assert_eq!(
            stream_fault_action(StreamFault::Other),
            StreamFaultAction::Reconnect(RecordingErrorCode::DeviceDisconnected)
        );
    }

    /// The seam this module exists for: "fatal" here must mean exactly what the
    /// recovery policy treats as fatal, or a `Reconnect` code would end the
    /// session (or a `Fatal` one would be retried for four hours).
    #[test]
    fn fatal_and_reconnect_agree_with_the_recovery_policy() {
        for f in ALL {
            match stream_fault_action(f) {
                StreamFaultAction::KeepRecording => {}
                StreamFaultAction::Reconnect(code) => {
                    assert!(!is_fatal_reconnect_error(code), "{f:?} → {code:?}")
                }
                StreamFaultAction::Fatal(code) => {
                    assert!(is_fatal_reconnect_error(code), "{f:?} → {code:?}")
                }
            }
        }
    }

    #[test]
    fn only_an_invalidated_stream_releases_the_rate_pin() {
        assert!(!reconnect_keeps_rate_pin(Some(
            StreamFault::StreamInvalidated
        )));
        assert!(reconnect_keeps_rate_pin(None));
        for f in ALL
            .into_iter()
            .filter(|f| *f != StreamFault::StreamInvalidated)
        {
            assert!(reconnect_keeps_rate_pin(Some(f)), "{f:?}");
        }
    }
}
