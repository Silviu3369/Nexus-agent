//! Automations (`schema::V10`): an instruction the user wrote, run as a new
//! task when its schedule is due, and one history entry per due time. The
//! store keeps the next due time the Host computed; it never computes
//! schedules itself. Events never carry the instruction.

use nexus_protocol::{AutomationId, AutomationRunStatus, ConversationId, ProjectId, Schedule};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::json;

use crate::repo::{insert_event, parse_uuid};
use crate::{StorageError, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRow {
    pub id: AutomationId,
    pub name: String,
    pub prompt: String,
    pub project_id: Option<ProjectId>,
    pub schedule: Schedule,
    pub enabled: bool,
    pub catch_up: bool,
    /// `None` when it will not run again (finished, or disabled).
    pub next_run_at_ms: Option<i64>,
    pub revision: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// What the user saves; the Host has checked it and computed `next_run_at_ms`.
#[derive(Debug, Clone)]
pub struct NewAutomation {
    pub name: String,
    pub prompt: String,
    pub project_id: Option<ProjectId>,
    pub schedule: Schedule,
    pub enabled: bool,
    pub catch_up: bool,
    pub next_run_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRunRow {
    pub automation_id: AutomationId,
    pub scheduled_for_ms: i64,
    pub at_ms: i64,
    pub status: AutomationRunStatus,
    pub reason: Option<String>,
    pub conversation_id: Option<ConversationId>,
    pub catch_up: bool,
    pub manual: bool,
}

const AUTOMATION_COLUMNS: &str = "id, name, prompt, project_id, schedule_json, enabled, catch_up,
    next_run_at_ms, revision, created_at_ms, updated_at_ms";
const RUN_COLUMNS: &str =
    "automation_id, scheduled_for_ms, at_ms, status, reason, conversation_id, catch_up, manual";
const MAX_REASON_CHARS: usize = 500;

fn status_str(status: AutomationRunStatus) -> &'static str {
    match status {
        AutomationRunStatus::Started => "Started",
        AutomationRunStatus::Skipped => "Skipped",
        AutomationRunStatus::Missed => "Missed",
        AutomationRunStatus::Failed => "Failed",
    }
}

fn parse_status(s: &str) -> Result<AutomationRunStatus, StorageError> {
    Ok(match s {
        "Started" => AutomationRunStatus::Started,
        "Skipped" => AutomationRunStatus::Skipped,
        "Missed" => AutomationRunStatus::Missed,
        "Failed" => AutomationRunStatus::Failed,
        other => {
            return Err(StorageError::Corrupt(format!(
                "unknown automation run status {other:?}"
            )));
        }
    })
}

impl Store {
    pub async fn automations(&self) -> Result<Vec<AutomationRow>, StorageError> {
        self.call(|conn| {
            let tx = conn.transaction()?;
            let mut stmt = tx.prepare(&format!(
                "SELECT {AUTOMATION_COLUMNS} FROM automations ORDER BY created_at_ms, id"
            ))?;
            let raw = stmt
                .query_map([], raw_automation)?
                .collect::<Result<Vec<_>, _>>()?;
            raw.into_iter().map(RawAutomation::parse).collect()
        })
        .await
    }

    pub async fn automation(&self, id: AutomationId) -> Result<AutomationRow, StorageError> {
        self.call(move |conn| load_automation(&conn.transaction()?, id))
            .await
    }

    /// Enabled automations whose next time has come.
    pub async fn due_automations(&self, now: i64) -> Result<Vec<AutomationRow>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let mut stmt = tx.prepare(&format!(
                "SELECT {AUTOMATION_COLUMNS} FROM automations
                 WHERE enabled = 1 AND next_run_at_ms IS NOT NULL AND next_run_at_ms <= ?1
                 ORDER BY next_run_at_ms, id"
            ))?;
            let raw = stmt
                .query_map([now], raw_automation)?
                .collect::<Result<Vec<_>, _>>()?;
            raw.into_iter().map(RawAutomation::parse).collect()
        })
        .await
    }

    pub async fn create_automation(
        &self,
        new: NewAutomation,
        now: i64,
    ) -> Result<AutomationRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let id = AutomationId::new();
            tx.execute(
                "INSERT INTO automations (id, name, prompt, project_id, schedule_json, enabled, catch_up,
                 next_run_at_ms, revision, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9)",
                params![
                    id.to_string(),
                    new.name,
                    new.prompt,
                    new.project_id.map(|p| p.to_string()),
                    schedule_json(&new.schedule)?,
                    new.enabled,
                    new.catch_up,
                    new.next_run_at_ms,
                    now
                ],
            )?;
            insert_event(&tx, "AutomationCreated", &id.to_string(), 1, json!({"enabled": new.enabled}), now)?;
            crate::actors::add_automation(&tx, id, &new.name, now)?;
            let row = load_automation(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Replaces an automation as the user edited it; refused if it changed
    /// since `revision` (an edit elsewhere is not overwritten).
    pub async fn update_automation(
        &self,
        id: AutomationId,
        revision: i64,
        new: NewAutomation,
        now: i64,
    ) -> Result<AutomationRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_automation(&tx, id)?;
            if current.revision != revision {
                return Err(StorageError::RevisionConflict("automation"));
            }
            tx.execute(
                "UPDATE automations SET name = ?1, prompt = ?2, project_id = ?3, schedule_json = ?4,
                 enabled = ?5, catch_up = ?6, next_run_at_ms = ?7, revision = revision + 1,
                 updated_at_ms = ?8 WHERE id = ?9",
                params![
                    new.name,
                    new.prompt,
                    new.project_id.map(|p| p.to_string()),
                    schedule_json(&new.schedule)?,
                    new.enabled,
                    new.catch_up,
                    new.next_run_at_ms,
                    now,
                    id.to_string()
                ],
            )?;
            crate::actors::rename(&tx, crate::Actor::Automation(id), &new.name)?;
            insert_event(
                &tx,
                "AutomationUpdated",
                &id.to_string(),
                revision + 1,
                json!({"enabled": new.enabled}),
                now,
            )?;
            let row = load_automation(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Removes the automation and its history; the tasks it started stay.
    pub async fn delete_automation(&self, id: AutomationId, now: i64) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_automation(&tx, id)?;
            tx.execute("DELETE FROM automations WHERE id = ?1", [id.to_string()])?;
            crate::actors::disable(&tx, crate::Actor::Automation(id), now)?;
            insert_event(
                &tx,
                "AutomationDeleted",
                &id.to_string(),
                current.revision + 1,
                json!({}),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Records what happened at due times and moves the automation on to
    /// `next_run_at_ms`, in one transaction. The move happens only while
    /// the automation still expects `expected_next` (an edit made meanwhile
    /// keeps its own next time); `None` expects nothing (a manual run, which
    /// does not move the schedule). Returns false, recording nothing, when
    /// the automation was deleted meanwhile.
    pub async fn record_automation_runs(
        &self,
        id: AutomationId,
        expected_next: Option<i64>,
        runs: Vec<AutomationRunRow>,
        next_run_at_ms: Option<i64>,
        now: i64,
    ) -> Result<bool, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let Some(current) = try_load_automation(&tx, id)? else {
                return Ok(false);
            };
            for run in &runs {
                let reason = run
                    .reason
                    .as_deref()
                    .map(|r| r.chars().take(MAX_REASON_CHARS).collect::<String>());
                tx.execute(
                    &format!("INSERT INTO automation_runs ({RUN_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
                    params![
                        id.to_string(),
                        run.scheduled_for_ms,
                        run.at_ms,
                        status_str(run.status),
                        reason,
                        run.conversation_id.map(|c| c.to_string()),
                        run.catch_up,
                        run.manual
                    ],
                )?;
                insert_event(
                    &tx,
                    "AutomationRan",
                    &id.to_string(),
                    current.revision,
                    json!({
                        "status": status_str(run.status),
                        "scheduled_for_ms": run.scheduled_for_ms,
                        "conversation": run.conversation_id.map(|c| c.to_string()),
                        "catch_up": run.catch_up,
                        "manual": run.manual,
                    }),
                    now,
                )?;
            }
            if expected_next.is_some() && current.next_run_at_ms == expected_next {
                tx.execute(
                    "UPDATE automations SET next_run_at_ms = ?1 WHERE id = ?2",
                    params![next_run_at_ms, id.to_string()],
                )?;
            }
            tx.commit()?;
            Ok(true)
        })
        .await
    }

    /// Newest first.
    pub async fn automation_runs(
        &self,
        id: AutomationId,
        limit: u32,
    ) -> Result<Vec<AutomationRunRow>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            load_automation(&tx, id)?;
            load_runs(&tx, id, limit)
        })
        .await
    }

    /// The task already started for an idempotency key, if any.
    pub async fn conversation_with_key(
        &self,
        idempotency_key: String,
    ) -> Result<Option<ConversationId>, StorageError> {
        self.call(move |conn| {
            let id: Option<String> = conn
                .query_row(
                    "SELECT conversation_id FROM messages WHERE idempotency_key = ?1",
                    [idempotency_key],
                    |r| r.get(0),
                )
                .optional()?;
            id.map(|id| Ok(ConversationId(parse_uuid(&id)?)))
                .transpose()
        })
        .await
    }

    /// A reply in this task is still being written.
    pub async fn conversation_answering(&self, id: ConversationId) -> Result<bool, StorageError> {
        self.call(move |conn| {
            let found: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM messages WHERE conversation_id = ?1 AND status = 'Streaming' LIMIT 1",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(found.is_some())
        })
        .await
    }
}

struct RawAutomation {
    id: String,
    name: String,
    prompt: String,
    project_id: Option<String>,
    schedule_json: String,
    enabled: bool,
    catch_up: bool,
    next_run_at_ms: Option<i64>,
    revision: i64,
    created_at_ms: i64,
    updated_at_ms: i64,
}

fn raw_automation(r: &rusqlite::Row) -> rusqlite::Result<RawAutomation> {
    Ok(RawAutomation {
        id: r.get(0)?,
        name: r.get(1)?,
        prompt: r.get(2)?,
        project_id: r.get(3)?,
        schedule_json: r.get(4)?,
        enabled: r.get(5)?,
        catch_up: r.get(6)?,
        next_run_at_ms: r.get(7)?,
        revision: r.get(8)?,
        created_at_ms: r.get(9)?,
        updated_at_ms: r.get(10)?,
    })
}

impl RawAutomation {
    fn parse(self) -> Result<AutomationRow, StorageError> {
        Ok(AutomationRow {
            id: AutomationId(parse_uuid(&self.id)?),
            name: self.name,
            prompt: self.prompt,
            project_id: self
                .project_id
                .map(|p| Ok::<_, StorageError>(ProjectId(parse_uuid(&p)?)))
                .transpose()?,
            schedule: serde_json::from_str(&self.schedule_json)
                .map_err(|e| StorageError::Corrupt(format!("automation schedule: {e}")))?,
            enabled: self.enabled,
            catch_up: self.catch_up,
            next_run_at_ms: self.next_run_at_ms,
            revision: self.revision,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
        })
    }
}

fn schedule_json(schedule: &Schedule) -> Result<String, StorageError> {
    serde_json::to_string(schedule)
        .map_err(|e| StorageError::Corrupt(format!("automation schedule: {e}")))
}

fn try_load_automation(
    tx: &Transaction,
    id: AutomationId,
) -> Result<Option<AutomationRow>, StorageError> {
    tx.query_row(
        &format!("SELECT {AUTOMATION_COLUMNS} FROM automations WHERE id = ?1"),
        [id.to_string()],
        raw_automation,
    )
    .optional()?
    .map(RawAutomation::parse)
    .transpose()
}

fn load_automation(tx: &Transaction, id: AutomationId) -> Result<AutomationRow, StorageError> {
    try_load_automation(tx, id)?.ok_or(StorageError::NotFound("automation"))
}

fn load_runs(
    tx: &Transaction,
    id: AutomationId,
    limit: u32,
) -> Result<Vec<AutomationRunRow>, StorageError> {
    let mut stmt = tx.prepare(&format!(
        "SELECT {RUN_COLUMNS} FROM automation_runs WHERE automation_id = ?1 ORDER BY id DESC LIMIT ?2"
    ))?;
    let raw = stmt
        .query_map(params![id.to_string(), limit], |r| {
            Ok((
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, bool>(6)?,
                r.get::<_, bool>(7)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    raw.into_iter()
        .map(
            |(scheduled_for_ms, at_ms, status, reason, conversation, catch_up, manual)| {
                Ok(AutomationRunRow {
                    automation_id: id,
                    scheduled_for_ms,
                    at_ms,
                    status: parse_status(&status)?,
                    reason,
                    conversation_id: conversation
                        .map(|c| Ok::<_, StorageError>(ConversationId(parse_uuid(&c)?)))
                        .transpose()?,
                    catch_up,
                    manual,
                })
            },
        )
        .collect()
}
