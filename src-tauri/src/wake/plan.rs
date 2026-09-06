//! Pure command PLANS for the wake shell — what we are about to run, decided
//! without running anything.
//!
//! Every OS invocation in [`super`] used to be an inline `run("pmset", &[…])`
//! literal. That made the *argument shaping* — including the AppleScript literal
//! the elevated macOS path builds by string concatenation — untestable: the only
//! way to see what would be executed was to execute it. Here each invocation is
//! a [`PlannedCommand`] built by a `plan_*` function, so a test can assert on the
//! exact program and argv, and the two quoting layers the osascript path needs
//! (POSIX shell, then AppleScript string literal) are named functions with
//! golden tests instead of backslashes buried in a `format!`.
//!
//! The same split as [`crate::recorder::native_capture::stream::pick_input_config`]:
//! a pure decision, then a thin executor.

use chrono::NaiveDateTime;

use sundayrec_core::wake::format_pmset_date;

/// The label `pmset` files our scheduled wakes under. `cancelall` matches on it,
/// so the schedule and the cancel MUST use the same string.
pub const WAKE_OWNER: &str = "SundayRec";

/// An OS command we intend to run: program, argv, and the timeout after which we
/// give up on it. Built by the `plan_*` functions, executed by a
/// [`super::shell::Shell`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedCommand {
    pub program: String,
    pub args: Vec<String>,
    pub timeout_ms: u64,
}

impl PlannedCommand {
    fn new(program: &str, args: &[&str], timeout_ms: u64) -> Self {
        Self {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            timeout_ms,
        }
    }

    /// `program arg arg …` — the form a [`super::shell::FakeShell`] rule matches
    /// against, and what we log. NOT a re-runnable shell line (no quoting): it is
    /// a description, never fed back to a shell.
    pub fn rendered(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Quoting — the seam where a stray character escapes its context
// ─────────────────────────────────────────────────────────────────────────────

/// POSIX single-quote one shell word so the shell sees it as a single literal
/// argument, whatever it contains.
///
/// Single quotes protect everything except a single quote itself, which is
/// closed, escaped, and reopened (`'` → `'\''`). Spaces, `"`, `$`, `` ` ``,
/// `;`, `&&`, newlines and every non-ASCII byte pass through untouched — UTF-8
/// needs no escaping inside single quotes, so `æøå` survives verbatim.
pub fn shell_quote(word: &str) -> String {
    let mut out = String::with_capacity(word.len() + 2);
    out.push('\'');
    for ch in word.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Wrap `s` in an AppleScript double-quoted string literal, escaping every
/// character that can end the literal early or break the line.
///
/// AppleScript recognises exactly `\\`, `\"`, `\n`, `\r` and `\t` inside a
/// string literal, and has no encoding rule for anything else — so Norwegian
/// text passes through as itself while a raw newline (which would otherwise end
/// the statement) becomes an escape. Backslash MUST be handled first: escaping
/// the quote before the backslash would leave a trailing `\` free to swallow the
/// closing `"`.
pub fn applescript_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

// ─────────────────────────────────────────────────────────────────────────────
//   macOS plans
// ─────────────────────────────────────────────────────────────────────────────

/// Cancel every wake we previously filed under [`WAKE_OWNER`].
pub fn plan_mac_cancel_all() -> PlannedCommand {
    PlannedCommand::new("pmset", &["schedule", "cancelall", WAKE_OWNER], 3_000)
}

/// Schedule one wake, un-elevated (works when the user already has the right, or
/// when `pmset` has been granted it; otherwise it fails and we escalate).
pub fn plan_mac_schedule_one(d: NaiveDateTime) -> PlannedCommand {
    PlannedCommand::new(
        "pmset",
        &["schedule", "wake", &format_pmset_date(d), WAKE_OWNER],
        5_000,
    )
}

/// The elevated fallback: ONE `osascript` admin prompt that runs every `pmset`
/// call, chained with `&&` so a failure stops the rest.
///
/// Two quoting layers stack here, and both are applied explicitly:
///  1. each `pmset` argument is [`shell_quote`]d, because `do shell script` hands
///     its string to `/bin/sh`;
///  2. the whole shell line is then wrapped by [`applescript_literal`], because
///     osascript parses it as AppleScript source first.
///
/// `pmset` is named by absolute path: `do shell script` runs with a minimal
/// environment, and a bare name would depend on whatever `PATH` it inherits.
pub fn plan_mac_elevated_schedule(points: &[NaiveDateTime], owner: &str) -> PlannedCommand {
    let cmds = points
        .iter()
        .map(|d| {
            format!(
                "/usr/bin/pmset schedule wake {} {}",
                shell_quote(&format_pmset_date(*d)),
                shell_quote(owner)
            )
        })
        .collect::<Vec<_>>()
        .join(" && ");
    plan_mac_admin_shell(&cmds, 30_000)
}

/// Disable autopoweroff and raise standbydelay so the Mac stays in a wakeable
/// sleep instead of dropping to hibernation. Needs the same admin prompt.
pub fn plan_mac_fix_sleep() -> PlannedCommand {
    plan_mac_admin_shell(
        "/usr/bin/pmset -a autopoweroff 0 && /usr/bin/pmset -a standbydelay 86400",
        30_000,
    )
}

/// `osascript -e 'do shell script "…" with administrator privileges'`.
fn plan_mac_admin_shell(shell_line: &str, timeout_ms: u64) -> PlannedCommand {
    let script = format!(
        "do shell script {} with administrator privileges",
        applescript_literal(shell_line)
    );
    PlannedCommand::new("osascript", &["-e", &script], timeout_ms)
}

/// `pmset -g` — the full power configuration (standby, autopoweroff, hibernate).
pub fn plan_mac_sleep_config() -> PlannedCommand {
    PlannedCommand::new("pmset", &["-g"], 5_000)
}

/// `pmset -g sched` — the TEXT view of scheduled power events. Only a fallback
/// now; [`super::mac_read`] reads the same data through IOKit first.
pub fn plan_mac_sched() -> PlannedCommand {
    PlannedCommand::new("pmset", &["-g", "sched"], 5_000)
}

/// `pmset -g batt` — AC or battery.
pub fn plan_mac_batt() -> PlannedCommand {
    PlannedCommand::new("pmset", &["-g", "batt"], 5_000)
}

// ─────────────────────────────────────────────────────────────────────────────
//   Windows plans
// ─────────────────────────────────────────────────────────────────────────────

/// The GUIDs of the "Sleep → Allow wake timers" setting in the active scheme.
const WAKE_TIMERS_SUBGROUP: &str = "238C9FA8-0AAD-41ED-83F4-97BE242C8F20";
const WAKE_TIMERS_SETTING: &str = "BD3B718A-0680-4D9D-8AB2-E1D2B4AC806D";

/// Read whether wake timers are allowed on AC in the active power scheme.
///
/// This matters MORE under the `SetWaitableTimer` mechanism than it did under
/// scheduled tasks: an armed timer with wake timers disabled fires without
/// resuming the machine, and nothing about the arming call reports that.
pub fn plan_win_wake_timers_query() -> PlannedCommand {
    let cmd = format!(
        "$s = (powercfg /getactivescheme) -replace '.*GUID: ([\\w-]+).*','$1'; powercfg /query $s {WAKE_TIMERS_SUBGROUP} {WAKE_TIMERS_SETTING}"
    );
    PlannedCommand::new("powershell", &["-NoProfile", "-Command", &cmd], 10_000)
}

/// Turn "Allow wake timers" on for both AC and battery in the active scheme.
pub fn plan_win_fix_wake_timers() -> PlannedCommand {
    let cmd = format!(
        "$s = (powercfg /getactivescheme) -replace '.*GUID: ([\\w-]+).*','$1'; powercfg /setacvalueindex $s {WAKE_TIMERS_SUBGROUP} {WAKE_TIMERS_SETTING} 1; powercfg /setdcvalueindex $s {WAKE_TIMERS_SUBGROUP} {WAKE_TIMERS_SETTING} 1; powercfg /setactive $s"
    );
    PlannedCommand::new(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &cmd],
        15_000,
    )
}

/// `powercfg -waketimers` — what the OS believes is armed, us included.
pub fn plan_win_waketimers() -> PlannedCommand {
    PlannedCommand::new("powercfg", &["-waketimers"], 5_000)
}

/// Battery status via `wmic` — absent on Windows 11 24H2 and later.
pub fn plan_win_battery_wmic() -> PlannedCommand {
    PlannedCommand::new(
        "wmic",
        &["path", "Win32_Battery", "get", "BatteryStatus", "/value"],
        5_000,
    )
}

/// Battery status via the CIM cmdlet — the fallback when `wmic` is gone.
pub fn plan_win_battery_cim() -> PlannedCommand {
    PlannedCommand::new(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-CimInstance -ClassName Win32_Battery | Select-Object -First 1 -ExpandProperty BatteryStatus)",
        ],
        8_000,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn shell_quote_neutralises_every_metacharacter() {
        assert_eq!(shell_quote("05/31/26 10:30:00"), "'05/31/26 10:30:00'");
        // The one character single quotes cannot contain is spliced out and back.
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        // A metacharacter salad stays one word: no unquoted `;`, `&&`, `$` or
        // backtick survives to be interpreted.
        let hostile = shell_quote("a; rm -rf / && $(id) `whoami`");
        assert_eq!(hostile, "'a; rm -rf / && $(id) `whoami`'");
        assert_eq!(hostile.matches('\'').count(), 2);
    }

    #[test]
    fn applescript_literal_escapes_quotes_and_backslashes() {
        assert_eq!(applescript_literal("plain"), "\"plain\"");
        assert_eq!(applescript_literal("say \"hi\""), "\"say \\\"hi\\\"\"");
        // Backslash first, or a trailing `\` would escape the closing quote and
        // the literal would run on into the rest of the script.
        assert_eq!(applescript_literal("ends\\"), "\"ends\\\\\"");
        assert_eq!(applescript_literal("a\\\"b"), "\"a\\\\\\\"b\"");
    }

    #[test]
    fn applescript_literal_passes_norwegian_characters_through() {
        // AppleScript literals have no escape rule for non-ASCII; osascript takes
        // UTF-8. A wake label with «æøå» must appear verbatim, NOT mangled and
        // not escaped — anything else would change the string pmset files under.
        assert_eq!(
            applescript_literal("Gudstjeneste på Sørøya æøå"),
            "\"Gudstjeneste på Sørøya æøå\""
        );
        assert_eq!(shell_quote("Sørøya æøå"), "'Sørøya æøå'");
    }

    #[test]
    fn mac_elevated_plan_is_byte_exact_for_a_normal_schedule() {
        // Golden: the ONE string handed to osascript for two wake points. If the
        // quoting layers are ever reordered or dropped this literal changes.
        let plan = plan_mac_elevated_schedule(
            &[dt("2026-05-31 10:20:00"), dt("2026-06-07 10:20:00")],
            WAKE_OWNER,
        );
        assert_eq!(plan.program, "osascript");
        assert_eq!(plan.args[0], "-e");
        assert_eq!(
            plan.args[1],
            "do shell script \"/usr/bin/pmset schedule wake '05/31/26 10:20:00' 'SundayRec' && /usr/bin/pmset schedule wake '06/07/26 10:20:00' 'SundayRec'\" with administrator privileges"
        );
    }

    #[test]
    fn mac_elevated_plan_keeps_a_hostile_owner_label_inside_the_literal() {
        // The classic seam bug: a quote in the owner label closes the AppleScript
        // string, and everything after it becomes AppleScript source. Feed the
        // builder a label engineered to do exactly that and assert it stays data.
        let plan = plan_mac_elevated_schedule(
            &[dt("2026-05-31 10:20:00")],
            "Sunday\" with administrator privileges\nrm -rf ~ --",
        );
        let script = &plan.args[1];
        // Exactly one unescaped `"` opens the literal and one closes it: every
        // other quote in the script is preceded by a backslash.
        let unescaped_quotes = script
            .char_indices()
            .filter(|(i, c)| *c == '"' && (*i == 0 || script.as_bytes()[i - 1] != b'\\'))
            .count();
        assert_eq!(unescaped_quotes, 2, "literal broken open by: {script}");
        // The privilege phrase appears once — ours — not the injected copy as
        // live AppleScript.
        assert!(script.ends_with("\" with administrator privileges"));
        assert!(script.contains("with administrator privileges\\n"));
    }

    #[test]
    fn mac_schedule_and_cancel_agree_on_the_owner_label() {
        // `cancelall` matches by label, so a drift between these two would leave
        // every previously-scheduled wake in place and silently stack duplicates.
        let cancel = plan_mac_cancel_all();
        let schedule = plan_mac_schedule_one(dt("2026-05-31 10:20:00"));
        assert_eq!(cancel.args, vec!["schedule", "cancelall", "SundayRec"]);
        assert_eq!(
            schedule.args,
            vec!["schedule", "wake", "05/31/26 10:20:00", "SundayRec"]
        );
        assert_eq!(cancel.args.last(), schedule.args.last());
    }

    #[test]
    fn win_power_plans_name_the_wake_timer_setting() {
        let q = plan_win_wake_timers_query();
        let fix = plan_win_fix_wake_timers();
        assert!(q.rendered().contains(WAKE_TIMERS_SETTING));
        // The fix must set BOTH the AC and the DC index — a laptop that only got
        // the AC half still refuses to wake on battery.
        assert!(fix.rendered().contains("setacvalueindex"));
        assert!(fix.rendered().contains("setdcvalueindex"));
        assert!(fix.rendered().contains("setactive"));
    }
}
