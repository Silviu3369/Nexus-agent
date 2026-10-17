//! Automations (`PLAN_NEXUS_RO.md` M4): an instruction the user wrote, sent
//! as a new task when its schedule is due, exactly as if the user had sent
//! it (the active model, proposals waiting for approval, the daily token
//! budget). The executor checks every tick. A time missed while the Host
//! was not running (or the PC slept) is caught up at most once, only when
//! the user asked for it and within a day; a run whose reply is still being
//! written is not overlapped. Each due time sends at most one task: its
//! idempotency key names the automation and the time.

use std::sync::Arc;

use nexus_protocol::{
    AutomationId, AutomationRunStatus, AutomationRunView, AutomationView, ConversationId,
    ErrorCode, NexusError, SaveAutomationRequest, Schedule, SendMessageRequest,
};
use nexus_storage::{AutomationRow, AutomationRunRow, NewAutomation, now_ms};

use super::chat::MAX_MESSAGE_BYTES;
use super::{Runtime, to_nexus_error};
use crate::schedule;

const MAX_NAME_CHARS: usize = 100;
/// Later than this after its time, a run counts as missed: the Host was
/// not running then, or the PC was asleep.
const ON_TIME_MS: i64 = 5 * 60 * 1000;
/// The oldest missed time still caught up.
const CATCH_UP_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_RUNS_PAGE: u32 = 200;
/// Due times counted one by one after a long stop.
const MAX_COUNTED: u32 = 10_000;
/// How far back a still-answering run is looked for.
const RECENT_RUNS: u32 = 50;

fn invalid(message: impl Into<String>) -> NexusError {
    NexusError::new(ErrorCode::InvalidRequest, message)
}

fn key_for(id: AutomationId, scheduled_for_ms: i64) -> String {
    format!("auto:{id}:{scheduled_for_ms}")
}

fn run_view(row: &AutomationRunRow) -> AutomationRunView {
    AutomationRunView {
        scheduled_for_ms: row.scheduled_for_ms,
        at_ms: row.at_ms,
        status: row.status,
        reason: row.reason.clone(),
        conversation_id: row.conversation_id,
        catch_up: row.catch_up,
        manual: row.manual,
    }
}

fn view(row: AutomationRow, last_run: Option<AutomationRunView>) -> AutomationView {
    AutomationView {
        automation_id: row.id,
        name: row.name,
        prompt: row.prompt,
        project_id: row.project_id,
        schedule: row.schedule,
        enabled: row.enabled,
        catch_up: row.catch_up,
        next_run_at_ms: row.next_run_at_ms,
        last_run,
        revision: row.revision,
        created_at_ms: row.created_at_ms,
    }
}

/// What came of one due time.
struct Outcome {
    status: AutomationRunStatus,
    reason: Option<String>,
    conversation_id: Option<ConversationId>,
}

impl Outcome {
    fn row(
        self,
        id: AutomationId,
        scheduled_for_ms: i64,
        catch_up: bool,
        manual: bool,
    ) -> AutomationRunRow {
        AutomationRunRow {
            automation_id: id,
            scheduled_for_ms,
            at_ms: now_ms(),
            status: self.status,
            reason: self.reason,
            conversation_id: self.conversation_id,
            catch_up,
            manual,
        }
    }
}

fn missed(id: AutomationId, scheduled_for_ms: i64, reason: String) -> AutomationRunRow {
    Outcome {
        status: AutomationRunStatus::Missed,
        reason: Some(reason),
        conversation_id: None,
    }
    .row(id, scheduled_for_ms, false, false)
}

impl Runtime {
    pub async fn list_automations(&self) -> Result<Vec<AutomationView>, NexusError> {
        let rows = self.store.automations().await.map_err(to_nexus_error)?;
        let mut views = Vec::with_capacity(rows.len());
        for row in rows {
            let last = self
                .store
                .automation_runs(row.id, 1)
                .await
                .map_err(to_nexus_error)?;
            views.push(view(row, last.first().map(run_view)));
        }
        Ok(views)
    }

    /// Creates or edits an automation; the next time is computed now, in
    /// this PC's local time.
    pub async fn save_automation(
        &self,
        request: SaveAutomationRequest,
    ) -> Result<AutomationView, NexusError> {
        let name = request.name.trim().to_string();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
            return Err(invalid(format!(
                "a name must have 1..={MAX_NAME_CHARS} characters"
            )));
        }
        let prompt = request.prompt.trim().to_string();
        if prompt.is_empty() || prompt.len() > MAX_MESSAGE_BYTES {
            return Err(invalid(format!(
                "the instruction must have 1..={MAX_MESSAGE_BYTES} bytes"
            )));
        }
        let schedule = match request.schedule {
            Schedule::Weekly {
                mut days,
                hour,
                minute,
            } => {
                days.sort_unstable();
                days.dedup();
                Schedule::Weekly { days, hour, minute }
            }
            once => once,
        };
        schedule::validate(&schedule).map_err(invalid)?;
        if let Some(project) = request.project_id {
            self.store.project(project).await.map_err(to_nexus_error)?;
        }
        let now = now_ms();
        let next_run_at_ms = if request.enabled {
            let next = schedule::next_run(&schedule, now, None);
            if next.is_none() {
                return Err(invalid(match schedule {
                    Schedule::Once { .. } => "that time has passed; choose a later one",
                    Schedule::Weekly { .. } => "the next time could not be computed",
                }));
            }
            next
        } else {
            None
        };
        let new = NewAutomation {
            name,
            prompt,
            project_id: request.project_id,
            schedule,
            enabled: request.enabled,
            catch_up: request.catch_up,
            next_run_at_ms,
        };
        let row = match request.automation_id {
            None => self.store.create_automation(new, now).await,
            Some(id) => {
                self.store
                    .update_automation(id, request.revision, new, now)
                    .await
            }
        }
        .map_err(to_nexus_error)?;
        let last = self
            .store
            .automation_runs(row.id, 1)
            .await
            .map_err(to_nexus_error)?;
        Ok(view(row, last.first().map(run_view)))
    }

    /// Removes the automation and its history; the tasks it started stay.
    pub async fn delete_automation(&self, id: AutomationId) -> Result<(), NexusError> {
        self.store
            .delete_automation(id, now_ms())
            .await
            .map_err(to_nexus_error)
    }

    pub async fn automation_runs(
        &self,
        id: AutomationId,
        limit: u32,
    ) -> Result<Vec<AutomationRunView>, NexusError> {
        let rows = self
            .store
            .automation_runs(id, limit.clamp(1, MAX_RUNS_PAGE))
            .await
            .map_err(to_nexus_error)?;
        Ok(rows.iter().map(run_view).collect())
    }

    /// Sends the instruction now, outside the schedule (which stays as it
    /// was). Refused while the previous run is still answering.
    pub async fn run_automation_now(
        self: &Arc<Self>,
        id: AutomationId,
    ) -> Result<AutomationRunView, NexusError> {
        if self.paused {
            return Err(NexusError::new(
                ErrorCode::Forbidden,
                "a restored profile does not run automations",
            ));
        }
        let row = self.store.automation(id).await.map_err(to_nexus_error)?;
        let now = now_ms();
        let outcome = self
            .start_run(&row, now, format!("auto:{id}:now:{now}"))
            .await?;
        if outcome.status == AutomationRunStatus::Skipped {
            return Err(invalid(
                "the previous run is still answering; wait for it or stop it first",
            ));
        }
        let run = outcome.row(id, now, false, true);
        let shown = run_view(&run);
        self.store
            .record_automation_runs(id, None, vec![run], None, now_ms())
            .await
            .map_err(to_nexus_error)?;
        Ok(shown)
    }

    /// Called by the executor every tick: each automation whose time has
    /// come runs (or is recorded as skipped, missed or failed) and moves on
    /// to its next time.
    pub(super) async fn run_due_automations(self: &Arc<Self>) {
        let due = match self.store.due_automations(now_ms()).await {
            Ok(due) => due,
            Err(err) => {
                tracing::warn!(error = %err, "could not read due automations");
                return;
            }
        };
        for row in due {
            if let Err(err) = self.run_due(&row).await {
                tracing::warn!(automation = %row.id, error = %err.message, "automation not run");
            }
        }
    }

    async fn run_due(self: &Arc<Self>, row: &AutomationRow) -> Result<(), NexusError> {
        let Some(first) = row.next_run_at_ms else {
            return Ok(());
        };
        let now = now_ms();
        // The Host stopped after sending this run and before recording it:
        // record the task it started, never send it again.
        if let Some(conversation) = self
            .store
            .conversation_with_key(key_for(row.id, first))
            .await
            .map_err(to_nexus_error)?
        {
            let started = Outcome {
                status: AutomationRunStatus::Started,
                reason: None,
                conversation_id: Some(conversation),
            };
            let next = schedule::next_run(&row.schedule, first, None);
            self.store
                .record_automation_runs(
                    row.id,
                    Some(first),
                    vec![started.row(row.id, first, now - first > ON_TIME_MS, false)],
                    next,
                    now,
                )
                .await
                .map_err(to_nexus_error)?;
            return Ok(());
        }

        // Every time that came while the Host was not running collapses
        // into the latest one; at most that one runs.
        let mut latest = first;
        let mut earlier = 0u32;
        let mut next = schedule::next_run(&row.schedule, first, None);
        while let Some(t) = next
            && t <= now
            && earlier < MAX_COUNTED
        {
            earlier += 1;
            latest = t;
            next = schedule::next_run(&row.schedule, t, None);
        }
        if next.is_some_and(|t| t <= now) {
            next = schedule::next_run(&row.schedule, now, None);
        }

        let mut runs = Vec::new();
        if earlier > 0 {
            let reason = match earlier {
                1 => "Nexus.Host was not running at this time".to_string(),
                2 => "Nexus.Host was not running at this time or the next one".to_string(),
                n => format!(
                    "Nexus.Host was not running at this time or the next {}",
                    n - 1
                ),
            };
            runs.push(missed(row.id, first, reason));
        }
        let late = now - latest;
        if late <= ON_TIME_MS || (row.catch_up && late <= CATCH_UP_WINDOW_MS) {
            let outcome = self.start_run(row, latest, key_for(row.id, latest)).await?;
            runs.push(outcome.row(row.id, latest, late > ON_TIME_MS, false));
        } else {
            let why = if row.catch_up {
                "Nexus.Host was not running at this time; it is more than a day ago, so it was not caught up"
            } else {
                "Nexus.Host was not running at this time; catching up is off"
            };
            runs.push(missed(row.id, latest, why.to_string()));
        }
        self.store
            .record_automation_runs(row.id, Some(first), runs, next, now_ms())
            .await
            .map_err(to_nexus_error)?;
        Ok(())
    }

    /// Sends the instruction as a new task, unless the previous run's reply
    /// is still being written. A failure to send is an outcome, not an error.
    async fn start_run(
        self: &Arc<Self>,
        row: &AutomationRow,
        scheduled_for_ms: i64,
        key: String,
    ) -> Result<Outcome, NexusError> {
        if let Some(conversation) = self
            .store
            .conversation_with_key(key.clone())
            .await
            .map_err(to_nexus_error)?
        {
            return Ok(Outcome {
                status: AutomationRunStatus::Started,
                reason: None,
                conversation_id: Some(conversation),
            });
        }
        let recent = self
            .store
            .automation_runs(row.id, RECENT_RUNS)
            .await
            .map_err(to_nexus_error)?;
        let previous = recent
            .iter()
            .find(|r| r.status == AutomationRunStatus::Started)
            .and_then(|r| r.conversation_id);
        if let Some(previous) = previous
            && self
                .store
                .conversation_answering(previous)
                .await
                .map_err(to_nexus_error)?
        {
            return Ok(Outcome {
                status: AutomationRunStatus::Skipped,
                reason: Some("the previous run was still answering".to_string()),
                conversation_id: None,
            });
        }
        let title = format!(
            "{} · {}",
            row.name,
            schedule::local_label(scheduled_for_ms, None)
        );
        let sent = self
            .start_message(
                SendMessageRequest {
                    conversation_id: None,
                    project_id: row.project_id,
                    idempotency_key: key,
                    text: row.prompt.clone(),
                },
                Some(title),
            )
            .await;
        Ok(match sent {
            Ok(sent) => Outcome {
                status: AutomationRunStatus::Started,
                reason: None,
                conversation_id: Some(sent.conversation_id),
            },
            Err(err) => Outcome {
                status: AutomationRunStatus::Failed,
                reason: Some(err.message),
                conversation_id: None,
            },
        })
    }
}
