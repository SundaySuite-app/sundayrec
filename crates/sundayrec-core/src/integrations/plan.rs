//! SundayPlan integration mappers.
//!
//! Pure port of `src/main/integrations/plan.ts` — the PULL side (turn a Plan
//! service into recording metadata + a schedule slot). The HTTP fetch/update is
//! the shell's (NETWORK-UNVERIFIED, reuses `reqwest`); the shaping decisions are
//! the unit-tested functions here.
//!
//! `service_to_schedule` is pure by taking an already-resolved `NaiveDateTime`
//! (the local wall-clock start). The Electron code did `new Date(starts_at_utc)`
//! then read local getters; the UTC→local conversion is a `clock`/timezone side
//! effect the shell owns, so it converts and hands us the naive local time. This
//! mirrors how the rest of `sundayrec-core` (filename/feed/schedule) keeps
//! `Local::now()` out of the testable core.

use chrono::{Datelike, NaiveDateTime, Timelike};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A planned service item (song / sermon / scripture …). Mirrors the Electron
/// `PlanServiceItem` (snake_case from the Supabase REST shape).
// mirrors src/main/integrations/plan.ts PlanServiceItem
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export, export_to = "../../../src/lib/bindings/PlanServiceItem.ts")]
pub struct PlanServiceItem {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment: Option<PlanAssignment>,
}

/// The speaker assignment on a sermon item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export, export_to = "../../../src/lib/bindings/PlanAssignment.ts")]
pub struct PlanAssignment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// A planned service from SundayPlan. Mirrors the Electron `PlanService`.
// mirrors src/main/integrations/plan.ts PlanService
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[ts(export, export_to = "../../../src/lib/bindings/PlanService.ts")]
pub struct PlanService {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub starts_at_utc: String,
    #[serde(default)]
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was_streamed_flag: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<Vec<PlanServiceItem>>,
}

/// Derived recording metadata. Mirrors `serviceToMetadata`'s return shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/PlanMetadata.ts")]
#[serde(rename_all = "camelCase")]
pub struct PlanMetadata {
    pub title: String,
    pub speaker: String,
}

/// A derived schedule slot. Mirrors `serviceToSchedule`'s return shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/PlanSchedule.ts")]
#[serde(rename_all = "camelCase")]
pub struct PlanSchedule {
    pub date: String,
    pub start_time: String,
    pub stop_time: String,
    pub note: String,
}

/// Derive a recording title + speaker. The title falls back to "Gudstjeneste"
/// when the service has no name; the speaker comes from the sermon item's
/// assignment (empty when none). Mirrors `serviceToMetadata`. Pure.
pub fn service_to_metadata(service: &PlanService) -> PlanMetadata {
    let title = if service.name.is_empty() {
        "Gudstjeneste".to_string()
    } else {
        service.name.clone()
    };
    let speaker = service
        .items
        .as_ref()
        .and_then(|items| items.iter().find(|i| i.kind == "sermon"))
        .and_then(|s| s.assignment.as_ref())
        .and_then(|a| a.speaker.clone())
        .unwrap_or_default();
    PlanMetadata { title, speaker }
}

/// Build a schedule slot from a service's *local* wall-clock start. Returns the
/// date (`YYYY-MM-DD`), start/stop times (`HH:MM`, a default 2-hour window), and
/// the service name as the note. Mirrors `serviceToSchedule`. The caller resolves
/// the UTC→local `NaiveDateTime` (a `clock` side effect); this stays pure.
pub fn service_to_schedule(service: &PlanService, local_start: NaiveDateTime) -> PlanSchedule {
    let stop = local_start + chrono::Duration::hours(2);
    PlanSchedule {
        date: format!(
            "{:04}-{:02}-{:02}",
            local_start.year(),
            local_start.month(),
            local_start.day()
        ),
        start_time: format!("{:02}:{:02}", local_start.hour(), local_start.minute()),
        stop_time: format!("{:02}:{:02}", stop.hour(), stop.minute()),
        note: service.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    #[test]
    fn metadata_defaults_title_and_pulls_sermon_speaker() {
        let svc = PlanService {
            id: "s1".into(),
            name: String::new(),
            items: Some(vec![
                PlanServiceItem {
                    id: "i1".into(),
                    kind: "song".into(),
                    ..Default::default()
                },
                PlanServiceItem {
                    id: "i2".into(),
                    kind: "sermon".into(),
                    assignment: Some(PlanAssignment {
                        speaker: Some("Ola Nordmann".into()),
                    }),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        let m = service_to_metadata(&svc);
        assert_eq!(m.title, "Gudstjeneste"); // empty name → default
        assert_eq!(m.speaker, "Ola Nordmann");
    }

    #[test]
    fn metadata_empty_speaker_without_sermon() {
        let svc = PlanService {
            id: "s1".into(),
            name: "Festgudstjeneste".into(),
            ..Default::default()
        };
        let m = service_to_metadata(&svc);
        assert_eq!(m.title, "Festgudstjeneste");
        assert_eq!(m.speaker, "");
    }

    #[test]
    fn schedule_uses_2_hour_window_and_name_note() {
        let svc = PlanService {
            id: "s1".into(),
            name: "Høymesse".into(),
            ..Default::default()
        };
        let sched = service_to_schedule(&svc, dt(2026, 5, 31, 11, 0));
        assert_eq!(sched.date, "2026-05-31");
        assert_eq!(sched.start_time, "11:00");
        assert_eq!(sched.stop_time, "13:00");
        assert_eq!(sched.note, "Høymesse");
    }

    #[test]
    fn schedule_window_wraps_past_midnight() {
        let svc = PlanService::default();
        let sched = service_to_schedule(&svc, dt(2026, 5, 31, 23, 30));
        assert_eq!(sched.start_time, "23:30");
        assert_eq!(sched.stop_time, "01:30"); // +2h wraps to next day's wall clock
    }
}
