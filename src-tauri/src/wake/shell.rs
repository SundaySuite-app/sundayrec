//! The one place the wake domain touches a process — behind a trait, so every
//! escalation ladder above it can be exercised without a `pmset` on the box.
//!
//! [`RealShell`] is the only implementation that spawns anything; the tests use
//! [`FakeShell`], which answers from a scripted rule list and records what it was
//! asked to run. Same shape as `telemetry::sender::TelemetrySender`: a hand-boxed
//! future rather than an `async-trait` proc macro, for one method.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::process::Command;

use super::plan::PlannedCommand;

/// What running a [`PlannedCommand`] produced. `code` is `None` when the process
/// was killed by a signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CmdOutput {
    /// Exit 0 with this stdout.
    pub fn ok(stdout: &str) -> Self {
        Self {
            code: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    /// A non-zero exit carrying this stderr.
    pub fn failed(code: i32, stderr: &str) -> Self {
        Self {
            code: Some(code),
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// The future [`Shell::run`] returns. Boxed by hand — see the module note.
pub type RunFuture<'a> = Pin<Box<dyn Future<Output = Result<CmdOutput, String>> + Send + 'a>>;

/// Run one planned command. `Err` means the process could not be run at all
/// (spawn failure, timeout); a command that ran and failed is `Ok` with a
/// non-zero [`CmdOutput::code`].
pub trait Shell: Send + Sync {
    fn run<'a>(&'a self, cmd: &'a PlannedCommand) -> RunFuture<'a>;
}

/// Collapse a run into the `Result<stdout, message>` contract the wake logic
/// works in: a non-zero exit becomes an `Err` carrying stderr (or a synthesised
/// description when the tool said nothing).
pub async fn run_text(shell: &dyn Shell, cmd: &PlannedCommand) -> Result<String, String> {
    match shell.run(cmd).await {
        Ok(out) if out.success() => Ok(out.stdout),
        Ok(out) => Err(if out.stderr.trim().is_empty() {
            match out.code {
                Some(c) => format!("{} exited with {c}", cmd.program),
                None => format!("{} was killed by a signal", cmd.program),
            }
        } else {
            out.stderr
        }),
        Err(e) => Err(e),
    }
}

/// Spawns the real process, with the plan's timeout.
#[derive(Debug, Default)]
pub struct RealShell;

impl Shell for RealShell {
    fn run<'a>(&'a self, cmd: &'a PlannedCommand) -> RunFuture<'a> {
        Box::pin(async move {
            let fut = Command::new(&cmd.program).args(&cmd.args).output();
            match tokio::time::timeout(Duration::from_millis(cmd.timeout_ms), fut).await {
                Ok(Ok(o)) => Ok(CmdOutput {
                    code: o.status.code(),
                    stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
                }),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err(format!("{} timed out", cmd.program)),
            }
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Test double
// ─────────────────────────────────────────────────────────────────────────────

/// A [`Shell`] that never spawns anything: it answers from rules matched against
/// [`PlannedCommand::rendered`] and records every call in order, so a test can
/// assert both what came back AND what was (or was not) attempted.
///
/// Rules are checked in insertion order; the first whose needle is a substring of
/// the rendered command wins. An unmatched command gets [`Self::default_reply`]
/// (exit 0, empty stdout, unless overridden).
#[cfg(test)]
pub struct FakeShell {
    rules: Vec<(String, Result<CmdOutput, String>)>,
    default_reply: Result<CmdOutput, String>,
    calls: std::sync::Mutex<Vec<PlannedCommand>>,
}

#[cfg(test)]
impl Default for FakeShell {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl FakeShell {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            default_reply: Ok(CmdOutput::ok("")),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Reply to any command whose rendered form contains `needle`.
    pub fn on(mut self, needle: &str, reply: Result<CmdOutput, String>) -> Self {
        self.rules.push((needle.to_string(), reply));
        self
    }

    /// What an unmatched command gets.
    pub fn otherwise(mut self, reply: Result<CmdOutput, String>) -> Self {
        self.default_reply = reply;
        self
    }

    /// Every command run so far, in order, rendered.
    pub fn log(&self) -> Vec<String> {
        crate::util::lock_recover(&self.calls)
            .iter()
            .map(|c| c.rendered())
            .collect()
    }

    /// How many recorded commands contain `needle`.
    pub fn count(&self, needle: &str) -> usize {
        self.log().iter().filter(|l| l.contains(needle)).count()
    }
}

#[cfg(test)]
impl Shell for FakeShell {
    fn run<'a>(&'a self, cmd: &'a PlannedCommand) -> RunFuture<'a> {
        let rendered = cmd.rendered();
        crate::util::lock_recover(&self.calls).push(cmd.clone());
        let reply = self
            .rules
            .iter()
            .find(|(needle, _)| rendered.contains(needle.as_str()))
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| self.default_reply.clone());
        Box::pin(async move { reply })
    }
}
