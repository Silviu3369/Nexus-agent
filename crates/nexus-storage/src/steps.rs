//! What the agent did while producing a reply (`schema::V4`): a model call,
//! a web read, a proposed note. Each step is written as Running when it
//! starts and settled exactly once, so the UI can show it live and later.
//! Visibility only — a step grants nothing and records no page content.

use nexus_core::{AgentStepKind, AgentStepStatus};
use nexus_protocol::{AgentStepId, ConversationId, MessageId};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Value, json};

use crate::repo::{insert_event, parse_state, parse_uuid};
use crate::{StorageError, Store};

#[derive(Debug, Clone, PartialEq)]
pub struct AgentStepRow {
    pub id: AgentStepId,
    pub message_id: MessageId,
    pub conversation_id: ConversationId,
    pub conversation_title: String,
    pub seq: i64,
    pub kind: AgentStepKind,
    pub status: AgentStepStatus,
    /// Kind-specific metadata (model, timings; URL, status, size; run id).
    pub detail: Value,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

const STEP_COLUMNS: &str =
    "s.id, s.message_id, m.conversation_id, c.title, s.seq, s.kind, s.status,
    s.detail_json, s.started_at_ms, s.finished_at_ms";
const STEP_FROM: &str = "FROM agent_steps s
    JOIN messages m ON m.id = s.message_id
    JOIN conversations c ON c.id = m.conversation_id";

impl Store {
    /// Records a step as Running on `message_id`, numbered after its
    /// previous steps.
    pub async fn begin_step(
        &self,
        message_id: MessageId,
        kind: AgentStepKind,
        detail: Value,
        now: i64,
    ) -> Result<AgentStepRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let seq: i64 = tx.query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM agent_steps WHERE message_id = ?1",
                [message_id.to_string()],
                |r| r.get(0),
            )?;
            let id = AgentStepId::new();
            tx.execute(
                "INSERT INTO agent_steps (id, message_id, seq, kind, status, detail_json, started_at_ms)
                 VALUES (?1, ?2, ?3, ?4, 'Running', ?5, ?6)",
                params![
                    id.to_string(),
                    message_id.to_string(),
                    seq,
                    kind.as_str(),
                    detail.to_string(),
                    now
                ],
            )?;
            insert_event(
                &tx,
                "AgentStepStarted",
                &id.to_string(),
                1,
                json!({"message": message_id.to_string(), "kind": kind.as_str()}),
                now,
            )?;
            let row = load_step(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Settles a Running step; `detail` is merged into what it started
    /// with (JSON merge patch), so the start's fields are kept.
    pub async fn finish_step(
        &self,
        id: AgentStepId,
        status: AgentStepStatus,
        detail: Value,
        now: i64,
    ) -> Result<AgentStepRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_step(&tx, id)?;
            if !current.status.can_transition_to(status) {
                return Err(StorageError::IllegalTransition(
                    nexus_core::IllegalTransition {
                        from: current.status.as_str(),
                        to: status.as_str(),
                    },
                ));
            }
            tx.execute(
                "UPDATE agent_steps SET status = ?1, detail_json = json_patch(detail_json, ?2), finished_at_ms = ?3
                 WHERE id = ?4 AND status = 'Running'",
                params![status.as_str(), detail.to_string(), now, id.to_string()],
            )?;
            insert_event(
                &tx,
                "AgentStepFinished",
                &id.to_string(),
                2,
                json!({"message": current.message_id.to_string(), "status": status.as_str()}),
                now,
            )?;
            let row = load_step(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    /// Fails every step still Running — of one message (its reply was
    /// stopped), or of all messages (startup: the host stopped mid-reply).
    pub async fn fail_running_steps(
        &self,
        message_id: Option<MessageId>,
        reason: &'static str,
        now: i64,
    ) -> Result<usize, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let patch = json!({ "error": reason }).to_string();
            let changed = match message_id {
                Some(id) => tx.execute(
                    "UPDATE agent_steps SET status = 'Failed', detail_json = json_patch(detail_json, ?1),
                     finished_at_ms = ?2 WHERE status = 'Running' AND message_id = ?3",
                    params![patch, now, id.to_string()],
                )?,
                None => tx.execute(
                    "UPDATE agent_steps SET status = 'Failed', detail_json = json_patch(detail_json, ?1),
                     finished_at_ms = ?2 WHERE status = 'Running'",
                    params![patch, now],
                )?,
            };
            if changed > 0 {
                insert_event(
                    &tx,
                    "AgentStepsInterrupted",
                    "agent_steps",
                    0,
                    json!({"count": changed, "reason": reason}),
                    now,
                )?;
            }
            tx.commit()?;
            Ok(changed)
        })
        .await
    }

    /// Steps of every message in a conversation, in order.
    pub async fn steps_for_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> Result<Vec<AgentStepRow>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let sql = format!(
                "SELECT {STEP_COLUMNS} {STEP_FROM} WHERE m.conversation_id = ?1 ORDER BY m.seq, s.seq"
            );
            let mut stmt = tx.prepare(&sql)?;
            let raws = stmt
                .query_map([conversation_id.to_string()], raw_step)?
                .collect::<Result<Vec<_>, _>>()?;
            raws.into_iter().map(step_from_raw).collect()
        })
        .await
    }

    /// The most recent steps across all conversations, newest first.
    pub async fn recent_steps(&self, limit: u32) -> Result<Vec<AgentStepRow>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let sql = format!(
                "SELECT {STEP_COLUMNS} {STEP_FROM} ORDER BY s.started_at_ms DESC, s.seq DESC LIMIT ?1"
            );
            let mut stmt = tx.prepare(&sql)?;
            let raws = stmt
                .query_map([limit], raw_step)?
                .collect::<Result<Vec<_>, _>>()?;
            raws.into_iter().map(step_from_raw).collect()
        })
        .await
    }
}

type RawStep = (
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
    i64,
    Option<i64>,
);

fn raw_step(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawStep> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
    ))
}

fn step_from_raw(raw: RawStep) -> Result<AgentStepRow, StorageError> {
    let (id, message_id, conversation_id, title, seq, kind, status, detail, started, finished) =
        raw;
    Ok(AgentStepRow {
        id: AgentStepId(parse_uuid(&id)?),
        message_id: MessageId(parse_uuid(&message_id)?),
        conversation_id: ConversationId(parse_uuid(&conversation_id)?),
        conversation_title: title,
        seq,
        kind: parse_state(&kind)?,
        status: parse_state(&status)?,
        detail: serde_json::from_str(&detail)
            .map_err(|e| StorageError::Corrupt(format!("step detail: {e}")))?,
        started_at_ms: started,
        finished_at_ms: finished,
    })
}

fn load_step(tx: &Transaction, id: AgentStepId) -> Result<AgentStepRow, StorageError> {
    let sql = format!("SELECT {STEP_COLUMNS} {STEP_FROM} WHERE s.id = ?1");
    let raw = tx
        .query_row(&sql, [id.to_string()], raw_step)
        .optional()?
        .ok_or(StorageError::NotFound("agent step"))?;
    step_from_raw(raw)
}
