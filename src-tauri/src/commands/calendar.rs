//! Liturgical-calendar commands — the thin IPC layer over the pure
//! `sundayrec_core::church_calendar` computus.
//!
//! Surfaces the Norwegian feast days ("Kirkehøytider") for a month so the
//! Tidsplan calendar can render them as blue "hoy"-kind events. Pure (no IO,
//! no managed State), so this is a plain sync command.

use serde::Serialize;
use ts_rs::TS;

use crate::error::AppResult;

/// One liturgical day in a month: its ISO date (`YYYY-MM-DD`) and the Norwegian
/// feast name from [`sundayrec_core::church_calendar::liturgical_day_name`].
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/LiturgicalDay.ts")]
pub struct LiturgicalDay {
    /// ISO date, `YYYY-MM-DD`.
    pub date: String,
    /// Norwegian liturgical day name (e.g. `"1. påskedag"`).
    pub name: String,
}

/// Every liturgical feast day in `(year, month)`, in date order.
///
/// Walks each valid day of the month, asks the pure church-calendar resolver
/// for a name, and keeps the days that have one. Ordinary days are skipped.
#[tauri::command]
pub fn liturgical_month(year: i32, month: u32) -> AppResult<Vec<LiturgicalDay>> {
    let mut out = Vec::new();
    for day in 1..=31 {
        let Some(date) = chrono::NaiveDate::from_ymd_opt(year, month, day) else {
            continue;
        };
        if let Some(name) = sundayrec_core::church_calendar::liturgical_day_name(date) {
            out.push(LiturgicalDay {
                date: date.format("%Y-%m-%d").to_string(),
                name,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_up_easter_week_2026() {
        // Easter 2026 = April 5; the month should include palmesøndag (Mar 29)
        // is in March, so April carries skjærtorsdag/langfredag/påskedag…
        let days = liturgical_month(2026, 4).unwrap();
        let names: Vec<&str> = days.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"1. påskedag"));
        assert!(names.contains(&"langfredag"));
        // Dates are ISO and in order.
        assert!(days.iter().any(|d| d.date == "2026-04-05"));
        assert!(days.windows(2).all(|w| w[0].date <= w[1].date));
    }

    #[test]
    fn ordinary_month_is_empty() {
        // No fixed or moveable feasts in a quiet stretch (June 2026 has none of
        // the configured days).
        assert!(liturgical_month(2026, 6).unwrap().is_empty());
    }

    #[test]
    fn invalid_month_yields_no_days() {
        // Month 13 has no valid dates → empty, no panic.
        assert!(liturgical_month(2026, 13).unwrap().is_empty());
    }
}
