//! DTOs for automations (`PLAN_NEXUS_RO.md` M4): an instruction the user
//! wrote, run as a new task on a schedule in this PC's local time, under
//! the same rules as a task the user starts (proposals wait for approval,
//! the daily token budget holds). A missed time is caught up at most once;
//! a run still answering is not overlapped.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::{AutomationId, ConversationId, ProjectId};

/// When an automation runs, in this PC's local time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind")]
pub enum Schedule {
    /// One moment (unix ms); afterwards the automation stays, finished.
    Once { at_ms: i64 },
    /// At this local time on the chosen days (0 = Monday … 6 = Sunday);
    /// every day when all seven are chosen.
    Weekly { days: Vec<u8>, hour: u8, minute: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum AutomationRunStatus {
    /// Sent as a task; its reply is in that task.
    Started,
    /// Not started: the previous run was still answering.
    Skipped,
    /// The time passed while the Host was not running, and catching up is
    /// off or the time is too old.
    Missed,
    /// The task could not be sent (no model, budget used up, …).
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AutomationRunView {
    /// The time it was due (unix ms).
    pub scheduled_for_ms: i64,
    pub at_ms: i64,
    pub status: AutomationRunStatus,
    /// Why it was skipped, missed or failed.
    pub reason: Option<String>,
    /// The task it started.
    pub conversation_id: Option<ConversationId>,
    /// Started late, catching up a time the Host was not running.
    pub catch_up: bool,
    /// Started by "Run now".
    pub manual: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AutomationView {
    pub automation_id: AutomationId,
    pub name: String,
    pub prompt: String,
    pub project_id: Option<ProjectId>,
    pub schedule: Schedule,
    pub enabled: bool,
    /// Run once at start-up for a time missed while the Host was off
    /// (within a day); never more than once.
    pub catch_up: bool,
    /// `None` when it will not run again (finished, or disabled).
    pub next_run_at_ms: Option<i64>,
    pub last_run: Option<AutomationRunView>,
    pub revision: i64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SaveAutomationRequest {
    /// `None` creates one.
    pub automation_id: Option<AutomationId>,
    /// The revision being edited (ignored on create): an edit made
    /// meanwhile elsewhere is not overwritten.
    pub revision: i64,
    pub name: String,
    pub prompt: String,
    /// `None`: the active project at the time of each run.
    pub project_id: Option<ProjectId>,
    pub schedule: Schedule,
    pub enabled: bool,
    pub catch_up: bool,
}
