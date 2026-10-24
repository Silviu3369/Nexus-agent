//! What the user did about the agent's work (ADR-TASK-001, `schema::V16`):
//! each intervention is written in the same transaction as the act itself
//! (a decision, an undo, a correction, a retry, a stop), attached to the
//! task and the turn it concerns; and the turns of a task are read with
//! them, so a turn's acceptance can be judged against its review window.

use nexus_core::{InterventionKind, RunState};
use nexus_protocol::{ConversationId, RunId};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Value, json};

use crate::actors::id_of;
use crate::repo::{insert_event, parse_state, parse_uuid};
use crate::{Actor, StorageError, Store};

/// The task and turn a message belongs to.
fn turn_of_message(
    tx: &Transaction,
    message: &str,
) -> Result<Option<(String, String)>, StorageError> {
    Ok(tx
        .query_row(
            "SELECT r.task_id, r.id FROM messages m JOIN runs r ON r.id = m.run_id WHERE m.id = ?1",
            [message],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// The task (and turn, if the message is known) of the agent's work in a
/// conversation.
pub(crate) fn turn_of(
    tx: &Transaction,
    conversation: Option<&str>,
    message: Option<&str>,
) -> Result<Option<(String, Option<String>)>, StorageError> {
    if let Some(message) = message
        && let Some((task, run)) = turn_of_message(tx, message)?
    {
        return Ok(Some((task, Some(run))));
    }
    let Some(conversation) = conversation else {
        return Ok(None);
    };
    Ok(tx
        .query_row(
            "SELECT id FROM tasks WHERE conversation_id = ?1",
            [conversation],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .map(|task| (task, None)))
}

/// An effect run's turn: its parent (or the run itself, for a note sent
/// directly), with that turn's task.
pub(crate) fn turn_of_effect(
    tx: &Transaction,
    effect_run: &str,
) -> Result<Option<(String, String)>, StorageError> {
    Ok(tx
        .query_row(
            "SELECT coalesce(p.task_id, e.task_id), coalesce(e.parent_run_id, e.id)
             FROM runs e LEFT JOIN runs p ON p.id = e.parent_run_id WHERE e.id = ?1",
            [effect_run],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Records one intervention by `actor` (and an event, so views refresh).
#[allow(clippy::too_many_arguments)]
pub(crate) fn record(
    tx: &Transaction,
    actor: Actor,
    task: &str,
    run: Option<&str>,
    effect_run: Option<&str>,
    kind: InterventionKind,
    detail: Value,
    now: i64,
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO interventions (task_id, run_id, effect_run_id, kind, detail_json, at_ms, actor_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            task,
            run,
            effect_run,
            kind.as_str(),
            detail.to_string(),
            now,
            id_of(tx, actor)?
        ],
    )?;
    insert_event(
        tx,
        "InterventionRecorded",
        task,
        0,
        json!({"kind": kind.as_str(), "run": run}),
        now,
    )?;
    Ok(())
}

/// Records an intervention on the effect run `effect` (the approval of a
/// proposal, its rejection, its cancellation, its undo).
pub(crate) fn record_on_effect(
    tx: &Transaction,
    actor: Actor,
    effect: &str,
    kind: InterventionKind,
    detail: Value,
    now: i64,
) -> Result<(), StorageError> {
    if let Some((task, turn)) = turn_of_effect(tx, effect)? {
        record(
            tx,
            actor,
            &task,
            Some(&turn),
            Some(effect),
            kind,
            detail,
            now,
        )?;
    }
    Ok(())
}

/// One turn of a task with what the user did about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnReviewRow {
    pub run_id: RunId,
    pub state: RunState,
    pub failure_cause: Option<String>,
    /// When the turn last changed (its end, once it ended).
    pub ended_at_ms: i64,
    /// The interventions on the turn, oldest first.
    pub interventions: Vec<(InterventionKind, i64)>,
}

fn reviews(
    tx: &Transaction,
    where_sql: &str,
    arg: &str,
) -> Result<Vec<TurnReviewRow>, StorageError> {
    let runs: Vec<(String, String, Option<String>, i64)> = {
        let mut stmt = tx.prepare(&format!(
            "SELECT r.id, r.state, r.failure_cause, r.updated_at_ms FROM runs r
             JOIN tasks t ON t.id = r.task_id WHERE r.kind = 'Agent' AND {where_sql}
             ORDER BY r.created_at_ms, r.rowid"
        ))?;
        stmt.query_map([arg], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<_, _>>()?
    };
    let mut stmt =
        tx.prepare("SELECT kind, at_ms FROM interventions WHERE run_id = ?1 ORDER BY at_ms, id")?;
    runs.into_iter()
        .map(|(id, state, failure_cause, ended_at_ms)| {
            let interventions = stmt
                .query_map([&id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|(kind, at)| Ok((parse_state(&kind)?, at)))
                .collect::<Result<_, StorageError>>()?;
            Ok(TurnReviewRow {
                run_id: RunId(parse_uuid(&id)?),
                state: parse_state(&state)?,
                failure_cause,
                ended_at_ms,
                interventions,
            })
        })
        .collect()
}

impl Store {
    /// Every turn of a conversation's task, with its interventions.
    pub async fn turn_reviews(
        &self,
        conversation: ConversationId,
    ) -> Result<Vec<TurnReviewRow>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            reviews(&tx, "t.conversation_id = ?1", &conversation.to_string())
        })
        .await
    }
}
