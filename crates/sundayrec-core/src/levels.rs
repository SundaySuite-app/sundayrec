//! The per-channel level value the meters and the channel diagnosis share.
//!
//! WHY: [`ChannelLevels`] is the small per-channel value the recording screen's
//! meters are fed (the native engine fills it from its own peaks) and the editor's
//! channel diagnosis reads (peak and RMS, measured in Rust in the open round by
//! [`crate::open_read::ChannelMeter`]). It used to be filled by parsing ffmpeg's
//! `astats` stderr; both of those parsers went with the `astats` round (B4b), and
//! with them the live `ametadata` line parser that went in C3b. What is left is the
//! value and the floor it maps silence to.

/// The latest per-channel levels, in dBFS (always ≤ 0 in normal use).
///
/// The `*_right` values are `None` for mono sources (one channel only). A
/// fully-silent / non-finite reading is mapped to [`SILENCE_FLOOR_DB`] so the
/// UI shows a pinned-low meter rather than `-inf`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelLevels {
    /// Peak level (dBFS) of channel 1 (left / the only channel on mono).
    pub peak_db_left: f64,
    /// Peak level (dBFS) of channel 2 (right), or `None` when the source is mono.
    pub peak_db_right: Option<f64>,
    /// RMS (average) level (dBFS) of channel 1, when the measurement carried one.
    /// `None` on the LIVE meter path, which reports peaks only.
    pub rms_db_left: Option<f64>,
    /// RMS (average) level (dBFS) of channel 2, same caveats as
    /// [`Self::rms_db_left`], plus `None` for mono.
    pub rms_db_right: Option<f64>,
}

impl ChannelLevels {
    /// Peaks only — the LIVE meter path, which has no RMS to report.
    pub fn peaks(peak_db_left: f64, peak_db_right: Option<f64>) -> Self {
        Self {
            peak_db_left,
            peak_db_right,
            rms_db_left: None,
            rms_db_right: None,
        }
    }
}

/// Floor used for `-inf` / `nan` / non-finite peak readings. Chosen well below
/// the meter's usable range so it reads as "silent" without being `-inf` (which
/// the UI's `formatDbfs` would render as `−∞`, but a numeric floor keeps the
/// segment math finite and steady).
pub const SILENCE_FLOOR_DB: f64 = -120.0;
