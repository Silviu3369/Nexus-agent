//! Actions outside the approval queue (ADR-ACTION-001): the agent's
//! register writes and proposed facts, recorded as actions of their turn in
//! the same transaction as the effect; and compensations (an undo), each
//! naming the action it reverses, with its intent written before the
//! effect.

use nexus_core::{ActionState, EffectClass, Evidence, InterventionKind};
use nexus_protocol::{ActionId, OperationId, RunId};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::repo::{insert_event, parse_state, parse_uuid, transition_action};
use crate::{StorageError, Store};

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Inserts an action already in `state` (one executed in this very
/// transaction, or a proposal waiting outside the approval queue).
#[allow(clippy::too_many_arguments)]
pub(crate) fn record(
    tx: &Transaction,
    run: &str,
    tool: &str,
    version: i64,
    effect: EffectClass,
    evidence: Evidence,
    state: ActionState,
    manifest: Value,
    compensates: Option<&str>,
    now: i64,
) -> Result<ActionId, StorageError> {
    let id = ActionId::new();
    let manifest_json = manifest.to_string();
    tx.execute(
        "INSERT INTO actions (id, run_id, operation_id, tool_id, tool_version, state, manifest_json,
                              manifest_sha256, revision, created_at_ms, updated_at_ms, effect_class, evidence,
                              compensates_action_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9, ?10, ?11, ?12)",
        params![
            id.to_string(),
            run,
            OperationId::new().to_string(),
            tool,
            version,
            state.as_str(),
            manifest_json,
            sha256_hex(manifest_json.as_bytes()),
            now,
            effect.as_str(),
            evidence.as_str(),
            compensates
        ],
    )?;
    insert_event(
        tx,
        "ActionRecorded",
        &id.to_string(),
        1,
        json!({"run_id": run, "tool": tool, "state": state.as_str(), "effect": effect.as_str()}),
        now,
    )?;
    Ok(id)
}

/// The turn (agent run) a message belongs to.
pub(crate) fn turn_of_message(
    tx: &Transaction,
    message: &str,
) -> Result<Option<String>, StorageError> {
    Ok(tx
        .query_row(
            "SELECT run_id FROM messages WHERE id = ?1",
            [message],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// The action recorded for one of the agent's register changes.
pub(crate) fn of_record_change(
    tx: &Transaction,
    change: i64,
) -> Result<Option<(String, String)>, StorageError> {
    Ok(tx
        .query_row(
            "SELECT id, run_id FROM actions WHERE tool_id = 'RecordWrite'
             AND json_extract(manifest_json, '$.change') = ?1",
            [change],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Settles the action of a fact the agent proposed: approved (it takes
/// effect, in this transaction) or rejected.
pub(crate) fn decide_fact(
    tx: &Transaction,
    memory: &str,
    approve: bool,
    now: i64,
) -> Result<(), StorageError> {
    let action: Option<String> = tx
        .query_row(
            "SELECT id FROM actions WHERE tool_id = 'MemoryFact' AND state = 'AwaitingApproval'
             AND json_extract(manifest_json, '$.memory') = ?1",
            [memory],
            |r| r.get(0),
        )
        .optional()?;
    let Some(action) = action else {
        return Ok(());
    };
    let id = ActionId(parse_uuid(&action)?);
    if approve {
        transition_action(tx, id, ActionState::Authorized, now)?;
        transition_action(tx, id, ActionState::Executing, now)?;
        transition_action(tx, id, ActionState::Verified, now)
    } else {
        transition_action(tx, id, ActionState::Denied, now)
    }
}

/// An undo of an effect run's action, with its intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompensationRow {
    pub action_id: ActionId,
    pub run_id: RunId,
    pub state: ActionState,
    pub manifest_json: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

fn compensation_from_row(
    r: &rusqlite::Row,
) -> rusqlite::Result<(String, String, String, String, i64, i64)> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
    ))
}

fn parse_compensation(
    raw: (String, String, String, String, i64, i64),
) -> Result<CompensationRow, StorageError> {
    Ok(CompensationRow {
        action_id: ActionId(parse_uuid(&raw.0)?),
        run_id: RunId(parse_uuid(&raw.1)?),
        state: parse_state(&raw.2)?,
        manifest_json: raw.3,
        created_at_ms: raw.4,
        updated_at_ms: raw.5,
    })
}

const COMPENSATION_COLUMNS: &str =
    "c.id, c.run_id, c.state, c.manifest_json, c.created_at_ms, c.updated_at_ms";

impl Store {
    /// Records the intent to undo the effect of run `run` (its action must
    /// be verified, and not undone or being undone) before anything is
    /// touched. The user's own request is its authorization.
    pub async fn begin_compensation(
        &self,
        run: RunId,
        tool: String,
        version: i64,
        effect: EffectClass,
        manifest: Value,
        now: i64,
    ) -> Result<ActionId, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let original: Option<(String, String)> = tx
                .query_row(
                    "SELECT id, state FROM actions WHERE run_id = ?1 AND compensates_action_id IS NULL",
                    [run.to_string()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((original, state)) = original else {
                return Err(StorageError::NotFound("action"));
            };
            if state != ActionState::Verified.as_str() {
                return Err(StorageError::NotUndoable(
                    "only an effect that was carried out and verified can be undone".into(),
                ));
            }
            let open: i64 = tx.query_row(
                "SELECT count(*) FROM actions WHERE compensates_action_id = ?1 AND state IN ('Executing', 'Verified')",
                [&original],
                |r| r.get(0),
            )?;
            if open > 0 {
                return Err(StorageError::NotUndoable("it was already undone".into()));
            }
            let id = record(
                &tx,
                &run.to_string(),
                &tool,
                version,
                effect,
                Evidence::Verified,
                ActionState::Executing,
                manifest,
                Some(&original),
                now,
            )?;
            tx.commit()?;
            Ok(id)
        })
        .await
    }

    /// Records how a compensation ended. Done: the run's outcome notes the
    /// undo, and the undo is the user's intervention on the turn.
    pub async fn finish_compensation(
        &self,
        action: ActionId,
        done: bool,
        reason: Option<String>,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let run: String = tx.query_row(
                "SELECT run_id FROM actions WHERE id = ?1 AND state = 'Executing'
                 AND compensates_action_id IS NOT NULL",
                [action.to_string()],
                |r| r.get(0),
            )?;
            if done {
                transition_action(&tx, action, ActionState::Verified, now)?;
                tx.execute(
                    "UPDATE runs SET outcome_json = json_set(coalesce(outcome_json, '{}'), '$.undone_at_ms', ?1),
                     updated_at_ms = ?1 WHERE id = ?2",
                    params![now, run],
                )?;
                crate::interventions::record_on_effect(
                    &tx,
                    &run,
                    InterventionKind::Undo,
                    json!({"action": action.to_string()}),
                    now,
                )?;
                insert_event(&tx, "EffectUndone", &run, 0, json!({"action": action.to_string()}), now)?;
            } else {
                transition_action(&tx, action, ActionState::Failed, now)?;
                insert_event(
                    &tx,
                    "EffectUndoFailed",
                    &run,
                    0,
                    json!({"action": action.to_string(), "reason": reason}),
                    now,
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Compensations whose intent was written but whose end was not (the
    /// Host stopped meanwhile): decided at startup from what the resource
    /// shows, never by repeating them blindly.
    pub async fn open_compensations(&self) -> Result<Vec<CompensationRow>, StorageError> {
        self.call(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {COMPENSATION_COLUMNS} FROM actions c
                 WHERE c.compensates_action_id IS NOT NULL AND c.state = 'Executing' ORDER BY c.created_at_ms"
            ))?;
            let rows = stmt
                .query_map([], compensation_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter().map(parse_compensation).collect()
        })
        .await
    }

    /// The latest compensation of a run's effect, if any.
    pub async fn compensation_of(
        &self,
        run: RunId,
    ) -> Result<Option<CompensationRow>, StorageError> {
        self.call(move |conn| {
            let raw = conn
                .query_row(
                    &format!(
                        "SELECT {COMPENSATION_COLUMNS} FROM actions c
                         WHERE c.run_id = ?1 AND c.compensates_action_id IS NOT NULL
                         ORDER BY c.created_at_ms DESC, c.rowid DESC LIMIT 1"
                    ),
                    [run.to_string()],
                    compensation_from_row,
                )
                .optional()?;
            raw.map(parse_compensation).transpose()
        })
        .await
    }

    /// The actions of a turn (its register writes, facts, and so on).
    pub async fn turn_actions(&self, run: RunId) -> Result<Vec<crate::ActionRow>, StorageError> {
        self.call(move |conn| crate::repo::actions_of_run(&conn.transaction()?, run))
            .await
    }
}
