//! ffmpeg capture-device fuzzy matching.
//!
//! Ported VERBATIM (logic, not structure) from the Electron `native-recorder.ts`
//! `findBestDeviceMatch` / `extractBrandWords` / `GENERIC_AUDIO_WORDS`
//! (lines 242–285). This is the *moat* against the real-world device chaos a
//! church A/V rig throws at us: the same Soundcraft/USB mixer can be reported
//! under a different name by the browser vs ffmpeg, by macOS vs Windows, and in
//! a different OS language. A stored device name from settings must still resolve
//! to the right ffmpeg device months later, across an OS update or a relabel.
//!
//! The five matching strategies, tried in order (first hit wins):
//!   1. **Exact** — case-insensitive equality.
//!   2. **Stored ⊂ device** — the stored name is a substring of a device name
//!      (e.g. "USB Audio" ⊂ "USB Audio Device (2- USB Audio)").
//!   3. **Device ⊂ stored** — the reverse (the device name is a substring of the
//!      stored name).
//!   4. **Word overlap ≥ 2** — handles localisation: the browser reports English
//!      ("MacBook Pro Microphone"), ffmpeg reports the OS language
//!      ("MacBook Pro-mikrofon"); they still share "macbook" + "pro".
//!   5. **Brand-word match** — strip the generic USB-audio vocabulary and the
//!      Windows "N- " prefix, then compare the remaining distinctive brand words
//!      ("Soundcraft USB Audio" vs "USB Audio CODEC").
//!
//! An empty stored name returns the first device (the OS default). No match at
//! all returns `None` so the caller can fail loudly rather than silently grab the
//! wrong microphone.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A capture device as enumerated from ffmpeg's device listing. Minimal on
/// purpose — matching only needs the human-readable `name`; `format`
/// (`"avfoundation"` / `"dshow"`) and the optional avfoundation `index` are
/// carried so the caller can build input args without a second lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/FfmpegDevice.ts")]
pub struct FfmpegDevice {
    /// Human-readable device name as ffmpeg reports it.
    pub name: String,
    /// Capture format this device belongs to (`"avfoundation"`, `"dshow"`, …).
    pub format: String,
    /// avfoundation device index (`Some(0)`, `Some(1)`, …) when known. dshow
    /// devices are addressed by name, so this is `None` there.
    pub index: Option<u32>,
}

impl FfmpegDevice {
    /// Convenience constructor for tests and call sites that only have a name.
    pub fn new(name: impl Into<String>, format: impl Into<String>, index: Option<u32>) -> Self {
        Self {
            name: name.into(),
            format: format.into(),
            index,
        }
    }
}

/// Generic USB-audio words that are too common to distinguish one device from
/// another. Stripped before brand-word comparison (strategy 5). Mirrors the
/// Electron `GENERIC_AUDIO_WORDS` set verbatim.
pub const GENERIC_AUDIO_WORDS: &[&str] = &[
    "usb",
    "audio",
    "codec",
    "device",
    "input",
    "output",
    "microphone",
    "speaker",
    "headset",
    "headphone",
    "sound",
    "card",
    "interface",
    "capture",
    "playback",
    "recording",
    "stereo",
    "mono",
    "digital",
    "analog",
];

/// True if `w` is a generic audio word (case-sensitive on already-lowercased
/// input, matching the Electron `Set.has` on a lowercased token).
fn is_generic(w: &str) -> bool {
    GENERIC_AUDIO_WORDS.contains(&w)
}

/// Strip the leading Windows `"N- "` enumeration prefix (e.g. `"2- USB Audio
/// CODEC"` → `"USB Audio CODEC"`). Accepts both ASCII `-` and the en-dash `–`
/// ffmpeg sometimes emits, matching the Electron regex `^\d+[-–]\s*`.
fn strip_windows_prefix(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut i = 0;
    // One or more leading digits.
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return s; // no leading digit → not the prefix pattern
    }
    // A single '-' (ASCII) or '–' (en-dash, 3 UTF-8 bytes).
    let rest = &s[i..];
    let rest = if let Some(r) = rest.strip_prefix('-') {
        r
    } else if let Some(r) = rest.strip_prefix('\u{2013}') {
        r
    } else {
        return s; // digits not followed by a dash → leave untouched
    };
    rest.trim_start()
}

/// Tokenise on the separator class ffmpeg/OS names use, dropping tokens ≤ 2
/// chars. Mirrors the Electron `split(/[\s\-()+]+/).filter(w => w.length > 2)`.
fn significant_words(s: &str) -> Vec<String> {
    s.split([' ', '\t', '\n', '\r', '-', '(', ')', '+'])
        .filter(|w| w.chars().count() > 2)
        .map(|w| w.to_lowercase())
        .collect()
}

/// Extract distinctive brand/model words: strip the Windows `"N- "` prefix, split
/// on the broader separator class (including `,` `/` `\`), drop tokens ≤ 2 chars
/// AND any [`GENERIC_AUDIO_WORDS`]. Mirrors the Electron `extractBrandWords`.
pub fn extract_brand_words(s: &str) -> Vec<String> {
    let cleaned = strip_windows_prefix(s);
    cleaned
        .split([' ', '\t', '\n', '\r', '-', '(', ')', '+', ',', '/', '\\'])
        .map(|w| w.to_lowercase())
        .filter(|w| w.chars().count() > 2 && !is_generic(w))
        .collect()
}

/// Find the best matching device for a stored `name`, applying the five-strategy
/// ladder. Returns `None` only when no strategy matched (and `name` was
/// non-empty). An empty `name` returns the first device (the OS default).
///
/// Borrows from `devices`; the returned reference lives as long as the slice.
pub fn find_best_device_match<'a>(
    devices: &'a [FfmpegDevice],
    name: &str,
) -> Option<&'a FfmpegDevice> {
    if name.is_empty() {
        return devices.first();
    }
    let n = name.to_lowercase();

    // 1. Exact (case-insensitive).
    if let Some(d) = devices.iter().find(|d| d.name.to_lowercase() == n) {
        return Some(d);
    }
    // 2. Stored name is a substring of device name.
    if let Some(d) = devices.iter().find(|d| d.name.to_lowercase().contains(&n)) {
        return Some(d);
    }
    // 3. Device name is a substring of stored name.
    if let Some(d) = devices.iter().find(|d| n.contains(&d.name.to_lowercase())) {
        return Some(d);
    }
    // 4. Word overlap ≥ 2 (prefix-aware, for localisation).
    let stored_words = significant_words(&n);
    if let Some(d) = devices.iter().find(|d| {
        let dev_words = significant_words(&d.name);
        let overlaps = stored_words
            .iter()
            .filter(|sw| {
                dev_words
                    .iter()
                    .any(|dw| dw.starts_with(sw.as_str()) || sw.starts_with(dw.as_str()))
            })
            .count();
        overlaps >= 2
    }) {
        return Some(d);
    }
    // 5. Brand-word match (after stripping generic vocabulary + Windows prefix).
    let stored_brand = extract_brand_words(&n);
    if !stored_brand.is_empty() {
        if let Some(d) = devices.iter().find(|d| {
            let dev_brand = extract_brand_words(&d.name);
            stored_brand.iter().any(|sw| {
                dev_brand.iter().any(|dw| {
                    dw == sw || dw.starts_with(sw.as_str()) || sw.starts_with(dw.as_str())
                })
            })
        }) {
            return Some(d);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str) -> FfmpegDevice {
        FfmpegDevice::new(name, "dshow", None)
    }

    fn avf(name: &str, idx: u32) -> FfmpegDevice {
        FfmpegDevice::new(name, "avfoundation", Some(idx))
    }

    #[test]
    fn exact_match_wins() {
        let devs = vec![dev("Built-in Microphone"), dev("RODE NT-USB")];
        let got = find_best_device_match(&devs, "RODE NT-USB").unwrap();
        assert_eq!(got.name, "RODE NT-USB");
    }

    #[test]
    fn exact_match_is_case_insensitive() {
        let devs = vec![dev("RODE NT-USB")];
        let got = find_best_device_match(&devs, "rode nt-usb").unwrap();
        assert_eq!(got.name, "RODE NT-USB");
    }

    #[test]
    fn stored_is_substring_of_device() {
        // "USB Audio" stored, device reported with extra qualifier.
        let devs = vec![dev("USB Audio Device (2- USB Audio)")];
        let got = find_best_device_match(&devs, "USB Audio").unwrap();
        assert_eq!(got.name, "USB Audio Device (2- USB Audio)");
    }

    #[test]
    fn device_is_substring_of_stored() {
        // Stored name is longer/more qualified than what ffmpeg now reports.
        let devs = vec![dev("Scarlett 2i2")];
        let got = find_best_device_match(&devs, "Focusrite Scarlett 2i2 USB").unwrap();
        assert_eq!(got.name, "Scarlett 2i2");
    }

    #[test]
    fn word_overlap_handles_localisation() {
        // Browser reported English; ffmpeg reports the Norwegian OS name. They
        // still share "macbook" + "pro" → ≥ 2 overlap (strategy 4).
        let devs = vec![
            avf("MacBook Pro-mikrofon", 0),
            avf("Soundcraft Signature 12", 1),
        ];
        let got = find_best_device_match(&devs, "MacBook Pro Microphone (Built-in)").unwrap();
        assert_eq!(got.name, "MacBook Pro-mikrofon");
    }

    #[test]
    fn brand_word_match_strips_generic_vocabulary() {
        // "Soundcraft" is the only distinctive word once the generic USB/audio
        // vocabulary is stripped; it must match the device carrying it.
        let devs = vec![
            dev("USB Audio CODEC"),
            dev("Soundcraft USB Audio (USB Audio)"),
        ];
        let got = find_best_device_match(&devs, "Soundcraft USB Audio").unwrap();
        assert_eq!(got.name, "Soundcraft USB Audio (USB Audio)");
    }

    #[test]
    fn brand_word_match_ignores_windows_numeric_prefix() {
        // Device carries the Windows "2- " enumeration prefix; stripping it must
        // still let the distinctive remainder match the stored generic name's
        // brand words. Here both reduce to nothing distinctive EXCEPT via the
        // prefix-strip path, so we use a brand word to anchor it.
        let devs = vec![dev("2- Yamaha AG06")];
        let got = find_best_device_match(&devs, "Yamaha AG06").unwrap();
        assert_eq!(got.name, "2- Yamaha AG06");
    }

    #[test]
    fn empty_name_returns_first_device() {
        let devs = vec![dev("First"), dev("Second")];
        let got = find_best_device_match(&devs, "").unwrap();
        assert_eq!(got.name, "First");
    }

    #[test]
    fn empty_name_with_no_devices_is_none() {
        let devs: Vec<FfmpegDevice> = vec![];
        assert!(find_best_device_match(&devs, "").is_none());
    }

    #[test]
    fn no_match_returns_none() {
        let devs = vec![dev("Built-in Microphone"), dev("HDMI Output")];
        assert!(find_best_device_match(&devs, "Soundcraft Signature 22").is_none());
    }

    #[test]
    fn generic_only_names_do_not_false_match_via_brand() {
        // Two purely-generic names share NO distinctive brand word, so strategy 5
        // must not match them. (They also don't substring/overlap.)
        let devs = vec![dev("Microphone Input")];
        // "Speaker Output" → brand words empty after stripping generics → None.
        assert!(find_best_device_match(&devs, "Speaker Output").is_none());
    }

    #[test]
    fn extract_brand_words_strips_prefix_and_generics() {
        assert_eq!(
            extract_brand_words("2- USB Audio CODEC"),
            Vec::<String>::new()
        );
        assert_eq!(
            extract_brand_words("Soundcraft USB Audio (USB Audio)"),
            vec!["soundcraft".to_string()]
        );
        assert_eq!(
            extract_brand_words("3– Yamaha Steinberg USB"),
            vec!["yamaha".to_string(), "steinberg".to_string()]
        );
    }

    #[test]
    fn priority_exact_beats_substring() {
        // An exact match later in the list must still win over an earlier
        // substring candidate.
        let devs = vec![dev("USB Audio Device"), dev("USB Audio")];
        let got = find_best_device_match(&devs, "USB Audio").unwrap();
        assert_eq!(got.name, "USB Audio");
    }

    #[test]
    fn match_preserves_format_and_index() {
        let devs = vec![avf("FaceTime HD Camera", 0), avf("Built-in Mic", 1)];
        let got = find_best_device_match(&devs, "Built-in Mic").unwrap();
        assert_eq!(got.format, "avfoundation");
        assert_eq!(got.index, Some(1));
    }
}
