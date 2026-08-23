//! Tray menu *model* — pure, GUI-free (PU-2 P2a).
//!
//! Ported from the Electron `src/main/tray.ts` (the behavioural spec). That file
//! mixed the *decisions* (which localized labels to show, whether the status row
//! is clickable, when to surface the review-queue callout, what each item does)
//! with Electron's `Menu.buildFromTemplate` + `Tray` icon side effects.
//!
//! Here we keep ONLY the decision: given the current [`TrayState`] + language,
//! produce the ordered list of [`TrayItem`]s and the tooltip + icon base. The
//! `src-tauri` shell (behind the `tray` feature) maps each [`TrayItem`] to a
//! `tauri::menu::MenuItem` and wires its [`TrayItem::action`] to the matching
//! command/event — so the menu's *shape* is unit-tested and the GUI layer is a
//! dumb projection.

/// The seven UI languages, matching `tray.ts` `TRAY_LABELS`. Unknown codes fall
/// back to Norwegian (the Electron default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayLang {
    No,
    En,
    De,
    Sv,
    Da,
    Pl,
    Fr,
}

impl TrayLang {
    pub fn from_code(code: Option<&str>) -> Self {
        match code.unwrap_or("no") {
            "en" => TrayLang::En,
            "de" => TrayLang::De,
            "sv" => TrayLang::Sv,
            "da" => TrayLang::Da,
            "pl" => TrayLang::Pl,
            "fr" => TrayLang::Fr,
            _ => TrayLang::No,
        }
    }
}

/// The live recorder/scheduler facts the menu reflects. Mirrors the module-level
/// mutable state in `tray.ts` (`isRecording`, `hasError`, `nextRecording`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrayState {
    pub is_recording: bool,
    pub has_error: bool,
    /// Pre-formatted short label of the next recording (e.g. "Sun 11:00"), or
    /// `None`. Wall-clock formatting is a shell concern; the core just places it.
    pub next_recording_label: Option<String>,
}

/// A stable identifier for what a menu item does. The shell switches on this to
/// wire the click (emit an event / call a command). Mirrors the distinct `click`
/// handlers in `tray.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Status row — only clickable (→ show window) when there's an error.
    ShowOnError,
    /// Non-interactive info row (next-recording line).
    None,
    OpenWindow,
    StartRecording,
    StopRecording,
    OpenRecordingsFolder,
    RunPreflight,
    RunDiagnostics,
    Quit,
}

/// One row of the tray menu (or a separator).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayItem {
    Separator,
    Item {
        label: String,
        action: TrayAction,
        /// Whether the row is clickable. A disabled row shows context (status,
        /// next-recording) but does nothing.
        enabled: bool,
    },
}

impl TrayItem {
    fn item(label: impl Into<String>, action: TrayAction, enabled: bool) -> Self {
        TrayItem::Item {
            label: label.into(),
            action,
            enabled,
        }
    }
}

/// The icon variant the tray should display, by precedence: recording > error >
/// idle. The shell maps this to a platform-specific asset name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayIcon {
    Recording,
    Error,
    Idle,
}

/// Pick the icon for the current state. Recording wins over error wins over idle
/// (matches `tray.ts` `base` selection).
pub fn icon_for(state: &TrayState) -> TrayIcon {
    if state.is_recording {
        TrayIcon::Recording
    } else if state.has_error {
        TrayIcon::Error
    } else {
        TrayIcon::Idle
    }
}

/// The status-badge colour (RGB) composited onto the base app icon for a
/// [`TrayIcon`] variant, or `None` for [`TrayIcon::Idle`] (the bare app icon).
///
/// Dedicated tray-icon assets were never bundled (docs/NEEDS-RICHARD.md PU-2),
/// so instead of shipping three PNGs the shell paints a small corner dot onto
/// the app's own icon at runtime — see [`with_status_badge`]. Red = recording
/// (the universal record colour), amber = something is wrong.
pub fn badge_rgb(icon: TrayIcon) -> Option<[u8; 3]> {
    match icon {
        TrayIcon::Recording => Some([0xE5, 0x39, 0x35]),
        TrayIcon::Error => Some([0xF5, 0xA6, 0x23]),
        TrayIcon::Idle => None,
    }
}

/// Composite an opaque circular status badge into the bottom-right corner of an
/// RGBA icon, returning a NEW buffer (the input is untouched, so the app's own
/// icon can be badged repeatedly without accumulating dots).
///
/// The badge is a filled disc of diameter `⌊min(w,h) * 0.44⌋` (never smaller
/// than 4 px), inset by one badge-radius' worth of margin, with a 1-px darker
/// rim so it stays visible against a light icon. Purely arithmetic — no image
/// crate, no assets, and unit-testable without a GUI.
///
/// Returns the input unchanged when `rgba` is not exactly `width * height * 4`
/// bytes (a malformed icon must never panic the tray).
pub fn with_status_badge(rgba: &[u8], width: u32, height: u32, badge: [u8; 3]) -> Vec<u8> {
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected || width == 0 || height == 0 {
        return rgba.to_vec();
    }
    let mut out = rgba.to_vec();
    let short = width.min(height) as f32;
    let diameter = (short * 0.44).floor().max(4.0);
    let r = diameter / 2.0;
    // Inset so the disc sits just inside the bottom-right corner.
    let cx = width as f32 - r - short * 0.06;
    let cy = height as f32 - r - short * 0.06;
    let rim = (r - 1.0).max(0.0);

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > r {
                continue;
            }
            // Inside the rim band → darken the badge colour for contrast.
            let px = ((y as usize) * (width as usize) + x as usize) * 4;
            let shade = if dist > rim { 0.55 } else { 1.0 };
            out[px] = (badge[0] as f32 * shade) as u8;
            out[px + 1] = (badge[1] as f32 * shade) as u8;
            out[px + 2] = (badge[2] as f32 * shade) as u8;
            out[px + 3] = 0xFF;
        }
    }
    out
}

/// The tooltip text. Mirrors `tray.ts` `updateTooltip`: a base line plus, when a
/// next recording is known, a "Neste opptak: <label>" line.
pub fn tooltip(state: &TrayState, lang: TrayLang) -> String {
    let base = match lang {
        TrayLang::No => "SundayRec — kjører i bakgrunnen",
        TrayLang::En => "SundayRec — running in background",
        TrayLang::De => "SundayRec — läuft im Hintergrund",
        TrayLang::Sv => "SundayRec — körs i bakgrunden",
        TrayLang::Da => "SundayRec — kører i baggrunden",
        TrayLang::Pl => "SundayRec — działa w tle",
        TrayLang::Fr => "SundayRec — s'exécute en arrière-plan",
    };
    match &state.next_recording_label {
        Some(next) => format!("{base}\n{}: {next}", next_label(lang)),
        None => base.to_string(),
    }
}

/// Build the ordered tray menu for `state` + `lang`. Order + clickability:
///   status → [next-recording info] → open → start/stop → open-folder →
///   diagnostics → quit.
///
/// The tray is a quick background-menu, so it carries ONE system action. The
/// status row already answers "is the system OK?" at a glance; when it isn't,
/// "Diagnoser system…" opens the full report. (Electron's menu also surfaced a
/// quick "check system now" preflight here, but next to the status row + the
/// diagnostics item it read as a confusing near-duplicate, so the tray drops it
/// — the in-app preflight button still exists. `RunPreflight` stays as an action
/// for that path.)
pub fn build_menu(state: &TrayState, lang: TrayLang) -> Vec<TrayItem> {
    let mut items = Vec::new();

    // Status row — clickable (show window) only on error.
    let status_label = if state.is_recording {
        recording_label(lang)
    } else if state.has_error {
        error_label(lang)
    } else {
        ready_label(lang)
    };
    items.push(TrayItem::item(
        status_label,
        TrayAction::ShowOnError,
        state.has_error,
    ));

    // Next-recording info line (only when not recording and known).
    if !state.is_recording {
        if let Some(next) = &state.next_recording_label {
            items.push(TrayItem::item(
                format!("{}: {next}", next_label(lang)),
                TrayAction::None,
                false,
            ));
        }
    }

    items.push(TrayItem::Separator);
    items.push(TrayItem::item(
        open_label(lang),
        TrayAction::OpenWindow,
        true,
    ));
    if state.is_recording {
        items.push(TrayItem::item(
            stop_label(lang),
            TrayAction::StopRecording,
            true,
        ));
    } else {
        items.push(TrayItem::item(
            start_label(lang),
            TrayAction::StartRecording,
            true,
        ));
    }

    items.push(TrayItem::Separator);
    items.push(TrayItem::item(
        open_folder_label(lang),
        TrayAction::OpenRecordingsFolder,
        true,
    ));
    items.push(TrayItem::item(
        diagnose_label(lang),
        TrayAction::RunDiagnostics,
        true,
    ));

    items.push(TrayItem::Separator);
    items.push(TrayItem::item(quit_label(lang), TrayAction::Quit, true));

    items
}

// ── localized labels (ported verbatim from tray.ts) ─────────────────────────

fn recording_label(l: TrayLang) -> &'static str {
    match l {
        TrayLang::No => "🔴 Tar opp…",
        TrayLang::En => "🔴 Recording…",
        TrayLang::De => "🔴 Aufnahme…",
        TrayLang::Sv => "🔴 Spelar in…",
        TrayLang::Da => "🔴 Optager…",
        TrayLang::Pl => "🔴 Nagrywa…",
        TrayLang::Fr => "🔴 Enregistrement…",
    }
}
fn error_label(l: TrayLang) -> &'static str {
    match l {
        TrayLang::No => "⚠️ Feil — klikk for detaljer",
        TrayLang::En => "⚠️ Error — click for details",
        TrayLang::De => "⚠️ Fehler — klicken für Details",
        TrayLang::Sv => "⚠️ Fel — klicka för detaljer",
        TrayLang::Da => "⚠️ Fejl — klik for detaljer",
        TrayLang::Pl => "⚠️ Błąd — kliknij po szczegóły",
        TrayLang::Fr => "⚠️ Erreur — cliquez pour détails",
    }
}
fn ready_label(l: TrayLang) -> &'static str {
    match l {
        TrayLang::No => "✅ Klar",
        TrayLang::En => "✅ Ready",
        TrayLang::De => "✅ Bereit",
        TrayLang::Sv => "✅ Klar",
        TrayLang::Da => "✅ Klar",
        TrayLang::Pl => "✅ Gotowy",
        TrayLang::Fr => "✅ Prêt",
    }
}
fn open_label(l: TrayLang) -> &'static str {
    match l {
        TrayLang::No => "Åpne SundayRec",
        TrayLang::En => "Open SundayRec",
        TrayLang::De => "SundayRec öffnen",
        TrayLang::Sv => "Öppna SundayRec",
        TrayLang::Da => "Åbn SundayRec",
        TrayLang::Pl => "Otwórz SundayRec",
        TrayLang::Fr => "Ouvrir SundayRec",
    }
}
fn stop_label(l: TrayLang) -> &'static str {
    match l {
        TrayLang::No => "Stopp opptak",
        TrayLang::En => "Stop recording",
        TrayLang::De => "Aufnahme stoppen",
        TrayLang::Sv => "Stoppa inspelning",
        TrayLang::Da => "Stop optagelse",
        TrayLang::Pl => "Zatrzymaj nagrywanie",
        TrayLang::Fr => "Arrêter l'enregistrement",
    }
}
fn start_label(l: TrayLang) -> &'static str {
    match l {
        TrayLang::No => "Start opptak nå",
        TrayLang::En => "Start recording now",
        TrayLang::De => "Aufnahme starten",
        TrayLang::Sv => "Starta inspelning nu",
        TrayLang::Da => "Start optagelse nu",
        TrayLang::Pl => "Rozpocznij nagrywanie",
        TrayLang::Fr => "Démarrer un enregistrement",
    }
}
fn quit_label(l: TrayLang) -> &'static str {
    match l {
        TrayLang::No => "Avslutt",
        TrayLang::En => "Quit",
        TrayLang::De => "Beenden",
        TrayLang::Sv => "Avsluta",
        TrayLang::Da => "Afslut",
        TrayLang::Pl => "Wyjdź",
        TrayLang::Fr => "Quitter",
    }
}
fn diagnose_label(l: TrayLang) -> &'static str {
    match l {
        TrayLang::No => "Diagnoser system…",
        TrayLang::En => "Run diagnostics…",
        TrayLang::De => "Diagnose starten…",
        TrayLang::Sv => "Kör diagnostik…",
        TrayLang::Da => "Kør diagnostik…",
        TrayLang::Pl => "Uruchom diagnostykę…",
        TrayLang::Fr => "Lancer le diagnostic…",
    }
}
fn open_folder_label(l: TrayLang) -> &'static str {
    match l {
        TrayLang::No => "Åpne lagringsmappe",
        TrayLang::En => "Open recordings folder",
        TrayLang::De => "Aufnahmeordner öffnen",
        TrayLang::Sv => "Öppna inspelningsmapp",
        TrayLang::Da => "Åbn optagelsesmappe",
        TrayLang::Pl => "Otwórz folder nagrań",
        TrayLang::Fr => "Ouvrir le dossier des enregistrements",
    }
}
fn next_label(l: TrayLang) -> &'static str {
    match l {
        TrayLang::No => "Neste opptak",
        TrayLang::En => "Next recording",
        TrayLang::De => "Nächste Aufnahme",
        TrayLang::Sv => "Nästa inspelning",
        TrayLang::Da => "Næste optagelse",
        TrayLang::Pl => "Następne nagranie",
        TrayLang::Fr => "Prochain enregistrement",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(items: &[TrayItem]) -> Vec<TrayAction> {
        items
            .iter()
            .filter_map(|i| match i {
                TrayItem::Item { action, .. } => Some(*action),
                TrayItem::Separator => None,
            })
            .collect()
    }

    #[test]
    fn idle_menu_offers_start() {
        let menu = build_menu(&TrayState::default(), TrayLang::En);
        let acts = actions(&menu);
        assert!(acts.contains(&TrayAction::StartRecording));
        assert!(!acts.contains(&TrayAction::StopRecording));
        // Status row is disabled when there's no error.
        assert_eq!(
            menu[0],
            TrayItem::Item {
                label: "✅ Ready".into(),
                action: TrayAction::ShowOnError,
                enabled: false,
            }
        );
    }

    #[test]
    fn tray_exposes_one_system_action_diagnostics_not_preflight() {
        // The status row + diagnostics cover "is the system OK?"; the old quick
        // preflight item was a confusing near-duplicate, so the tray drops it.
        let acts = actions(&build_menu(&TrayState::default(), TrayLang::No));
        assert!(acts.contains(&TrayAction::RunDiagnostics));
        assert!(
            !acts.contains(&TrayAction::RunPreflight),
            "tray must not show both 'check system' and 'diagnose system'"
        );
    }

    #[test]
    fn recording_menu_swaps_start_for_stop_and_hides_next_line() {
        let state = TrayState {
            is_recording: true,
            next_recording_label: Some("Sun 11:00".into()),
            ..Default::default()
        };
        let menu = build_menu(&state, TrayLang::No);
        let acts = actions(&menu);
        assert!(acts.contains(&TrayAction::StopRecording));
        assert!(!acts.contains(&TrayAction::StartRecording));
        // Next-recording info line is suppressed while recording.
        assert!(!acts.contains(&TrayAction::None));
        assert_eq!(
            menu[0].clone(),
            TrayItem::item("🔴 Tar opp…", TrayAction::ShowOnError, false)
        );
    }

    #[test]
    fn error_state_makes_status_row_clickable() {
        let state = TrayState {
            has_error: true,
            ..Default::default()
        };
        let menu = build_menu(&state, TrayLang::En);
        assert_eq!(
            menu[0],
            TrayItem::Item {
                label: "⚠️ Error — click for details".into(),
                action: TrayAction::ShowOnError,
                enabled: true,
            }
        );
        assert_eq!(icon_for(&state), TrayIcon::Error);
    }

    #[test]
    fn next_recording_info_row_appears_when_idle() {
        let state = TrayState {
            next_recording_label: Some("Sun 11:00".into()),
            ..Default::default()
        };
        let menu = build_menu(&state, TrayLang::En);
        let info = menu.iter().find_map(|i| match i {
            TrayItem::Item {
                label,
                action: TrayAction::None,
                enabled,
            } => Some((label.clone(), *enabled)),
            _ => None,
        });
        assert_eq!(info, Some(("Next recording: Sun 11:00".into(), false)));
    }

    #[test]
    fn menu_always_ends_with_quit() {
        for lang in [TrayLang::No, TrayLang::Fr, TrayLang::Pl] {
            let menu = build_menu(&TrayState::default(), lang);
            let last = menu.last().unwrap();
            assert!(matches!(
                last,
                TrayItem::Item {
                    action: TrayAction::Quit,
                    ..
                }
            ));
        }
    }

    #[test]
    fn tooltip_appends_next_recording_when_known() {
        let bare = tooltip(&TrayState::default(), TrayLang::En);
        assert_eq!(bare, "SundayRec — running in background");
        let with_next = tooltip(
            &TrayState {
                next_recording_label: Some("Sun 11:00".into()),
                ..Default::default()
            },
            TrayLang::En,
        );
        assert_eq!(
            with_next,
            "SundayRec — running in background\nNext recording: Sun 11:00"
        );
    }

    #[test]
    fn lang_defaults_to_norwegian() {
        assert_eq!(TrayLang::from_code(None), TrayLang::No);
        assert_eq!(TrayLang::from_code(Some("zz")), TrayLang::No);
        assert_eq!(TrayLang::from_code(Some("de")), TrayLang::De);
    }

    #[test]
    fn icon_precedence_recording_over_error() {
        let state = TrayState {
            is_recording: true,
            has_error: true,
            ..Default::default()
        };
        assert_eq!(icon_for(&state), TrayIcon::Recording);
        assert_eq!(icon_for(&TrayState::default()), TrayIcon::Idle);
    }

    // ── Live transitions (the menu is REBUILT on every state change now, so the
    //    model has to be correct at each step of a real session, not just for
    //    hand-built snapshots). ────────────────────────────────────────────────

    #[test]
    fn a_full_session_walks_start_stop_error_and_queue() {
        let lang = TrayLang::No;
        let mut state = TrayState {
            next_recording_label: Some("søn. 11:00".into()),
            ..Default::default()
        };

        // 1. Idle with a schedule → start + the info row.
        let acts = actions(&build_menu(&state, lang));
        assert!(acts.contains(&TrayAction::StartRecording));
        assert!(acts.contains(&TrayAction::None), "next-recording info row");
        assert_eq!(icon_for(&state), TrayIcon::Idle);

        // 2. Recording begins → stop replaces start, the info row goes.
        state.is_recording = true;
        let acts = actions(&build_menu(&state, lang));
        assert!(acts.contains(&TrayAction::StopRecording));
        assert!(!acts.contains(&TrayAction::StartRecording));
        assert!(!acts.contains(&TrayAction::None));
        assert_eq!(icon_for(&state), TrayIcon::Recording);

        // 3. Recording ends → start is offered again, icon back to idle.
        state.is_recording = false;
        let menu = build_menu(&state, lang);
        let acts = actions(&menu);
        assert!(acts.contains(&TrayAction::StartRecording));
        assert_eq!(icon_for(&state), TrayIcon::Idle);

        // 4. Something breaks → the status row becomes clickable, icon goes amber.
        state.has_error = true;
        let menu = build_menu(&state, lang);
        assert!(matches!(
            &menu[0],
            TrayItem::Item {
                action: TrayAction::ShowOnError,
                enabled: true,
                ..
            }
        ));
        assert_eq!(icon_for(&state), TrayIcon::Error);

        // 5. Cleared → back to the opening shape.
        state.has_error = false;
        assert_eq!(
            build_menu(&state, lang),
            build_menu(
                &TrayState {
                    next_recording_label: Some("søn. 11:00".into()),
                    ..Default::default()
                },
                lang
            )
        );
    }

    #[test]
    fn losing_the_next_label_only_drops_the_info_row() {
        let with = TrayState {
            next_recording_label: Some("søn. 11:00".into()),
            ..Default::default()
        };
        let without = TrayState::default();
        let a = build_menu(&with, TrayLang::No);
        let b = build_menu(&without, TrayLang::No);
        assert_eq!(a.len(), b.len() + 1);
        assert_eq!(
            actions(&b)
                .iter()
                .filter(|x| **x == TrayAction::None)
                .count(),
            0
        );
    }

    #[test]
    fn changing_language_rebuilds_every_label() {
        let state = TrayState {
            next_recording_label: Some("Sun 11:00".into()),
            ..Default::default()
        };
        let no = build_menu(&state, TrayLang::No);
        let fr = build_menu(&state, TrayLang::Fr);
        // Same SHAPE, different words — a language switch must be a pure relabel.
        assert_eq!(actions(&no), actions(&fr));
        assert_ne!(no, fr);
    }

    #[test]
    fn badge_colours_follow_the_icon_variant() {
        assert_eq!(badge_rgb(TrayIcon::Idle), None);
        assert!(badge_rgb(TrayIcon::Recording).is_some());
        assert!(badge_rgb(TrayIcon::Error).is_some());
        assert_ne!(badge_rgb(TrayIcon::Recording), badge_rgb(TrayIcon::Error));
    }

    #[test]
    fn status_badge_paints_the_corner_and_leaves_the_rest_alone() {
        // 32×32 fully transparent icon.
        let w = 32u32;
        let h = 32u32;
        let base = vec![0u8; (w * h * 4) as usize];
        let out = with_status_badge(&base, w, h, [0xE5, 0x39, 0x35]);
        assert_eq!(out.len(), base.len());

        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [out[i], out[i + 1], out[i + 2], out[i + 3]]
        };
        // Top-left is untouched…
        assert_eq!(at(0, 0), [0, 0, 0, 0]);
        // …and the bottom-right carries an opaque, reddish dot.
        let corner = at(w - 7, h - 7);
        assert_eq!(corner[3], 0xFF);
        assert!(corner[0] > corner[1] && corner[0] > corner[2]);
    }

    #[test]
    fn status_badge_rejects_a_malformed_buffer_instead_of_panicking() {
        let bogus = vec![1u8, 2, 3];
        assert_eq!(with_status_badge(&bogus, 32, 32, [1, 2, 3]), bogus);
        assert!(with_status_badge(&[], 0, 0, [1, 2, 3]).is_empty());
    }
}
