//! Registers and their records (`schema::V11`): tables Nexus keeps for the
//! user, with the history of every change. The store keeps values as given
//! (the Host has checked them against the register's fields) and copies the
//! status and follow-up date out of them for filtering. Every change is one
//! transaction with its history entry and event; events never carry values.

use std::collections::BTreeMap;

use nexus_core::{
    ActionState, EffectClass, Evidence, InterventionKind, PolicyDecision, RecordActor,
    RecordChangeKind,
};
use nexus_protocol::{ConversationId, MessageId, RecordId, RegisterId, RunId};
use rusqlite::{OptionalExtension, Transaction, params};
use serde_json::json;

use crate::repo::{insert_event, parse_state, parse_uuid};
use crate::{StorageError, Store};

pub type Values = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterRow {
    pub id: RegisterId,
    pub key: String,
    pub name: String,
    /// The Host's own format.
    pub schema_json: String,
    pub status_field: String,
    pub follow_up_field: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRow {
    pub id: RecordId,
    pub register_id: RegisterId,
    pub values: Values,
    pub status: Option<String>,
    pub follow_up_on: Option<String>,
    pub revision: i64,
    pub created_by: RecordActor,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordChangeRow {
    pub id: i64,
    pub record_id: RecordId,
    pub kind: RecordChangeKind,
    pub actor: RecordActor,
    pub before: Option<Values>,
    pub after: Values,
    pub revision_after: i64,
    pub conversation_id: Option<ConversationId>,
    pub message_id: Option<MessageId>,
    pub read_hosts: Vec<String>,
    pub undone_at_ms: Option<i64>,
    pub at_ms: i64,
    /// The agent of the task it was made in (`None`: Nexus, or the user).
    pub agent: Option<String>,
}

/// Where an agent's change came from.
#[derive(Debug, Clone)]
pub struct AgentOrigin {
    pub conversation_id: ConversationId,
    pub message_id: MessageId,
    /// Web hosts read in that reply before the change.
    pub read_hosts: Vec<String>,
    /// What the approval policy decided for the change (ADR-POLICY-001):
    /// written at once (`Auto`), or approved by the user in a batch (`Ask`).
    pub policy: PolicyDecision,
}

/// One change of a batch the user approved (ADR-POLICY-001): a new record,
/// or a record's new values, refused if it changed since the user saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchWrite {
    pub register_id: RegisterId,
    /// `None`: a new record.
    pub record_id: Option<RecordId>,
    /// The revision the user saw (for a change to a record).
    pub expected_revision: Option<i64>,
    pub values: Values,
}

/// Records to list. Dates are `YYYY-MM-DD`, compared as text.
#[derive(Debug, Clone, Default)]
pub struct RecordFilter {
    /// Any of these statuses; empty: all.
    pub statuses: Vec<String>,
    /// Text found in any value.
    pub text: Option<String>,
    /// Only records whose follow-up date is on or before this day and whose
    /// status is not one of `closed_statuses`.
    pub follow_up_by: Option<String>,
    pub closed_statuses: Vec<String>,
    /// A date field (validated key) within an inclusive range.
    pub date_range: Option<(String, Option<String>, Option<String>)>,
    pub limit: u32,
}

/// What a write did: the record as it is now and its history entry.
#[derive(Debug, Clone)]
pub struct RecordWrite {
    pub record: RecordRow,
    pub change_id: i64,
}

const RECORD_COLUMNS: &str =
    "id, register_id, values_json, status, follow_up_on, revision, created_by,
    created_at_ms, updated_at_ms";
const CHANGE_COLUMNS: &str = "id, record_id, kind, actor, before_json, after_json, revision_after,
    conversation_id, message_id, read_hosts_json, undone_at_ms, at_ms,
    (SELECT a.name FROM tasks t JOIN agents a ON a.id = t.agent_id
     WHERE t.conversation_id = record_changes.conversation_id LIMIT 1)";

impl Store {
    /// Creates a register, or brings its name and fields up to date.
    pub async fn ensure_register(
        &self,
        key: String,
        name: String,
        schema_json: String,
        status_field: String,
        follow_up_field: Option<String>,
        now: i64,
    ) -> Result<RegisterRow, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let existing: Option<String> = tx
                .query_row("SELECT id FROM registers WHERE key = ?1", [&key], |r| r.get(0))
                .optional()?;
            let id = match existing {
                Some(id) => {
                    tx.execute(
                        "UPDATE registers SET name = ?1, schema_json = ?2, status_field = ?3,
                         follow_up_field = ?4, updated_at_ms = ?5
                         WHERE id = ?6 AND (name != ?1 OR schema_json != ?2 OR status_field != ?3
                                            OR follow_up_field IS NOT ?4)",
                        params![name, schema_json, status_field, follow_up_field, now, id],
                    )?;
                    RegisterId(parse_uuid(&id)?)
                }
                None => {
                    let id = RegisterId::new();
                    tx.execute(
                        "INSERT INTO registers (id, key, name, schema_json, status_field, follow_up_field,
                         created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                        params![id.to_string(), key, name, schema_json, status_field, follow_up_field, now],
                    )?;
                    id
                }
            };
            let row = load_register(&tx, id)?;
            tx.commit()?;
            Ok(row)
        })
        .await
    }

    pub async fn registers(&self) -> Result<Vec<RegisterRow>, StorageError> {
        self.call(|conn| {
            let tx = conn.transaction()?;
            let ids: Vec<String> = {
                let mut stmt = tx.prepare("SELECT id FROM registers ORDER BY created_at_ms, id")?;
                stmt.query_map([], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            ids.iter()
                .map(|id| load_register(&tx, RegisterId(parse_uuid(id)?)))
                .collect()
        })
        .await
    }

    pub async fn register(&self, id: RegisterId) -> Result<RegisterRow, StorageError> {
        self.call(move |conn| load_register(&conn.transaction()?, id))
            .await
    }

    /// How many records it has, and how many open ones are due by `day`.
    pub async fn register_counts(
        &self,
        id: RegisterId,
        day: String,
        closed_statuses: Vec<String>,
    ) -> Result<(u32, u32), StorageError> {
        self.call(move |conn| {
            let total: i64 = conn.query_row(
                "SELECT count(*) FROM records WHERE register_id = ?1",
                [id.to_string()],
                |r| r.get(0),
            )?;
            let closed = serde_json::to_string(&closed_statuses).unwrap_or_else(|_| "[]".into());
            let due: i64 = conn.query_row(
                "SELECT count(*) FROM records WHERE register_id = ?1 AND follow_up_on IS NOT NULL
                 AND follow_up_on <= ?2
                 AND (status IS NULL OR status NOT IN (SELECT value FROM json_each(?3)))",
                params![id.to_string(), day, closed],
                |r| r.get(0),
            )?;
            Ok((
                u32::try_from(total).unwrap_or(u32::MAX),
                u32::try_from(due).unwrap_or(u32::MAX),
            ))
        })
        .await
    }

    /// Matching records, most recently changed first, and how many match.
    pub async fn records(
        &self,
        register_id: RegisterId,
        filter: RecordFilter,
    ) -> Result<(Vec<RecordRow>, u32), StorageError> {
        self.call(move |conn| {
            let mut clauses = vec!["register_id = ?1".to_string()];
            let mut args: Vec<String> = vec![register_id.to_string()];
            let arg = |value: String, args: &mut Vec<String>| {
                args.push(value);
                format!("?{}", args.len())
            };
            if !filter.statuses.is_empty() {
                let list = arg(
                    serde_json::to_string(&filter.statuses).unwrap_or_else(|_| "[]".into()),
                    &mut args,
                );
                clauses.push(format!("status IN (SELECT value FROM json_each({list}))"));
            }
            if let Some(text) = filter.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                let escaped = text
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_");
                let pattern = arg(format!("%{escaped}%"), &mut args);
                clauses.push(format!(
                    "EXISTS (SELECT 1 FROM json_each(values_json) WHERE value LIKE {pattern} ESCAPE '\\')"
                ));
            }
            if let Some(day) = filter.follow_up_by {
                let day = arg(day, &mut args);
                let closed = arg(
                    serde_json::to_string(&filter.closed_statuses).unwrap_or_else(|_| "[]".into()),
                    &mut args,
                );
                clauses.push(format!(
                    "follow_up_on IS NOT NULL AND follow_up_on <= {day}
                     AND (status IS NULL OR status NOT IN (SELECT value FROM json_each({closed})))"
                ));
            }
            if let Some((field, from, to)) = filter.date_range {
                let path = arg(format!("$.\"{field}\""), &mut args);
                if let Some(from) = from {
                    let from = arg(from, &mut args);
                    clauses.push(format!("json_extract(values_json, {path}) >= {from}"));
                }
                if let Some(to) = to {
                    let to = arg(to, &mut args);
                    clauses.push(format!("json_extract(values_json, {path}) <= {to}"));
                }
            }
            let filter_sql = clauses.join(" AND ");
            let total: i64 = conn.query_row(
                &format!("SELECT count(*) FROM records WHERE {filter_sql}"),
                rusqlite::params_from_iter(args.iter()),
                |r| r.get(0),
            )?;
            let limit = arg(filter.limit.clamp(1, 500).to_string(), &mut args);
            let mut stmt = conn.prepare(&format!(
                "SELECT {RECORD_COLUMNS} FROM records WHERE {filter_sql}
                 ORDER BY updated_at_ms DESC, id LIMIT CAST({limit} AS INTEGER)"
            ))?;
            let raw = stmt
                .query_map(rusqlite::params_from_iter(args.iter()), raw_record)?
                .collect::<Result<Vec<_>, _>>()?;
            let rows = raw
                .into_iter()
                .map(RawRecord::parse)
                .collect::<Result<Vec<_>, _>>()?;
            Ok((rows, u32::try_from(total).unwrap_or(u32::MAX)))
        })
        .await
    }

    pub async fn record(&self, id: RecordId) -> Result<RecordRow, StorageError> {
        self.call(move |conn| load_record(&conn.transaction()?, id))
            .await
    }

    pub async fn add_record(
        &self,
        register_id: RegisterId,
        values: Values,
        actor: RecordActor,
        origin: Option<AgentOrigin>,
        now: i64,
    ) -> Result<RecordWrite, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let register = load_register(&tx, register_id)?;
            let id = RecordId::new();
            let (status, follow_up) = derived(&register, &values);
            tx.execute(
                &format!("INSERT INTO records ({RECORD_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?7)"),
                params![
                    id.to_string(),
                    register_id.to_string(),
                    values_json(&values)?,
                    status,
                    follow_up,
                    actor.as_str(),
                    now
                ],
            )?;
            let change_id = insert_change(&tx, id, RecordChangeKind::Created, actor, None, &values, 1, origin.as_ref(), now)?;
            insert_event(&tx, "RecordAdded", &id.to_string(), 1, json!({"register": register.key, "actor": actor.as_str()}), now)?;
            let record = load_record(&tx, id)?;
            tx.commit()?;
            Ok(RecordWrite { record, change_id })
        })
        .await
    }

    /// Writes the changes of a batch the user approved and completes the
    /// batch's run, in one transaction: every change is saved and the run
    /// succeeds, or nothing is written (a record that changed or went since
    /// the user saw it refuses the whole batch). Returns each change's
    /// record, in order.
    pub async fn apply_record_batch(
        &self,
        run_id: RunId,
        writes: Vec<BatchWrite>,
        origin: AgentOrigin,
        now: i64,
    ) -> Result<Vec<RecordId>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let mut ids = Vec::with_capacity(writes.len());
            for write in &writes {
                let register = load_register(&tx, write.register_id)?;
                let id = match write.record_id {
                    None => {
                        let id = RecordId::new();
                        let (status, follow_up) = derived(&register, &write.values);
                        tx.execute(
                            &format!("INSERT INTO records ({RECORD_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, 1, 'Agent', ?6, ?6)"),
                            params![id.to_string(), register.id.to_string(), values_json(&write.values)?, status, follow_up, now],
                        )?;
                        insert_change(&tx, id, RecordChangeKind::Created, RecordActor::Agent, None, &write.values, 1, Some(&origin), now)?;
                        id
                    }
                    Some(id) => {
                        let current = load_record(&tx, id)?;
                        if current.register_id != register.id
                            || write.expected_revision != Some(current.revision)
                        {
                            return Err(StorageError::RevisionConflict("record"));
                        }
                        if current.values != write.values {
                            let revision = current.revision + 1;
                            write_values(&tx, &register, id, &write.values, revision, now)?;
                            insert_change(
                                &tx,
                                id,
                                RecordChangeKind::Updated,
                                RecordActor::Agent,
                                Some(&current.values),
                                &write.values,
                                revision,
                                Some(&origin),
                                now,
                            )?;
                        }
                        id
                    }
                };
                ids.push(id);
            }
            let outcome = json!({"records": ids.iter().map(ToString::to_string).collect::<Vec<_>>()});
            crate::repo::complete_outside(&tx, run_id, outcome.to_string(), "RecordBatchWritten", now)?;
            tx.commit()?;
            Ok(ids)
        })
        .await
    }

    /// Adds many records the user imported, in one transaction: all of them
    /// or none. Each gets its own history entry; one event names the count.
    pub async fn import_records(
        &self,
        register_id: RegisterId,
        rows: Vec<Values>,
        now: i64,
    ) -> Result<Vec<RecordId>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let register = load_register(&tx, register_id)?;
            let mut ids = Vec::with_capacity(rows.len());
            for values in &rows {
                let id = RecordId::new();
                let (status, follow_up) = derived(&register, values);
                tx.execute(
                    &format!("INSERT INTO records ({RECORD_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, 1, 'User', ?6, ?6)"),
                    params![id.to_string(), register_id.to_string(), values_json(values)?, status, follow_up, now],
                )?;
                insert_change(&tx, id, RecordChangeKind::Created, RecordActor::User, None, values, 1, None, now)?;
                ids.push(id);
            }
            insert_event(
                &tx,
                "RecordsImported",
                &register_id.to_string(),
                1,
                json!({"register": register.key, "count": ids.len()}),
                now,
            )?;
            tx.commit()?;
            Ok(ids)
        })
        .await
    }

    /// Replaces a record's values. `expected_revision` (the user's edit)
    /// refuses a record changed meanwhile; the agent writes on the latest.
    /// Writing the same values changes nothing and records no history.
    pub async fn update_record(
        &self,
        id: RecordId,
        expected_revision: Option<i64>,
        values: Values,
        actor: RecordActor,
        origin: Option<AgentOrigin>,
        now: i64,
    ) -> Result<Option<RecordWrite>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_record(&tx, id)?;
            if expected_revision.is_some_and(|r| r != current.revision) {
                return Err(StorageError::RevisionConflict("record"));
            }
            if current.values == values {
                return Ok(None);
            }
            let register = load_register(&tx, current.register_id)?;
            if actor == RecordActor::User {
                // Overwriting what the agent wrote corrects it (ADR-TASK-001).
                let changed: Vec<&String> = current
                    .values
                    .keys()
                    .chain(values.keys())
                    .filter(|k| current.values.get(*k) != values.get(*k))
                    .collect();
                for (change, fields) in agent_writers(&tx, id, Some(&changed))? {
                    agent_intervention(
                        &tx,
                        &change,
                        InterventionKind::Correction,
                        json!({"record": id.to_string(), "change": change.id, "fields": fields}),
                        now,
                    )?;
                }
            }
            let revision = current.revision + 1;
            write_values(&tx, &register, id, &values, revision, now)?;
            let change_id = insert_change(
                &tx,
                id,
                RecordChangeKind::Updated,
                actor,
                Some(&current.values),
                &values,
                revision,
                origin.as_ref(),
                now,
            )?;
            insert_event(
                &tx,
                "RecordUpdated",
                &id.to_string(),
                revision,
                json!({"register": register.key, "actor": actor.as_str()}),
                now,
            )?;
            let record = load_record(&tx, id)?;
            tx.commit()?;
            Ok(Some(RecordWrite { record, change_id }))
        })
        .await
    }

    /// Newest first.
    pub async fn record_history(&self, id: RecordId) -> Result<Vec<RecordChangeRow>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            load_record(&tx, id)?;
            let mut stmt = tx.prepare(&format!(
                "SELECT {CHANGE_COLUMNS} FROM record_changes WHERE record_id = ?1 ORDER BY id DESC"
            ))?;
            let raw = stmt
                .query_map([id.to_string()], raw_change)?
                .collect::<Result<Vec<_>, _>>()?;
            raw.into_iter().map(RawChange::parse).collect()
        })
        .await
    }

    /// A register's records (at most `max_records`, the newest) and every
    /// change to them, oldest first, read together for its results; plus
    /// whether some records were left out.
    pub async fn register_activity(
        &self,
        register_id: RegisterId,
        max_records: u32,
    ) -> Result<(Vec<RecordRow>, Vec<RecordChangeRow>, bool), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let total: i64 = tx.query_row(
                "SELECT count(*) FROM records WHERE register_id = ?1",
                [register_id.to_string()],
                |r| r.get(0),
            )?;
            let mut stmt = tx.prepare(&format!(
                "SELECT {RECORD_COLUMNS} FROM records WHERE register_id = ?1
                 ORDER BY created_at_ms DESC, id LIMIT ?2"
            ))?;
            let records = stmt
                .query_map(params![register_id.to_string(), max_records], raw_record)?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(RawRecord::parse)
                .collect::<Result<Vec<_>, _>>()?;
            let mut stmt = tx.prepare(&format!(
                "SELECT {CHANGE_COLUMNS} FROM record_changes
                 WHERE record_id IN (SELECT id FROM records WHERE register_id = ?1)
                 ORDER BY id"
            ))?;
            let changes = stmt
                .query_map([register_id.to_string()], raw_change)?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(RawChange::parse)
                .collect::<Result<Vec<_>, _>>()?;
            drop(stmt);
            Ok((records, changes, total > i64::from(max_records)))
        })
        .await
    }

    /// Reverses a record's latest change. Undoing its addition removes the
    /// record (returns `None`); undoing an update puts the earlier values
    /// back as a new revision. The agent step that made the change is marked
    /// undone.
    pub async fn undo_record_change(
        &self,
        change_id: i64,
        now: i64,
    ) -> Result<Option<RecordRow>, StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let change = tx
                .query_row(
                    &format!("SELECT {CHANGE_COLUMNS} FROM record_changes WHERE id = ?1"),
                    [change_id],
                    raw_change,
                )
                .optional()?
                .ok_or(StorageError::NotFound("record change"))?
                .parse()?;
            if change.undone_at_ms.is_some() {
                return Err(StorageError::NotUndoable("this change was already undone".into()));
            }
            if change.kind == RecordChangeKind::Undo {
                return Err(StorageError::NotUndoable(
                    "an undo is not undone; edit the record instead".into(),
                ));
            }
            let current = load_record(&tx, change.record_id)?;
            if current.revision != change.revision_after {
                return Err(StorageError::NotUndoable(
                    "the record changed after this; undo the later change first".into(),
                ));
            }
            if change.actor == RecordActor::Agent {
                if let Some((original, turn)) = crate::actions::of_record_change(&tx, change.id)? {
                    crate::actions::record(
                        &tx,
                        crate::Actor::LocalUser,
                        &turn,
                        "UndoRecordChange",
                        1,
                        EffectClass::InternalReversible,
                        Evidence::Verified,
                        ActionState::Verified,
                        json!({"record": change.record_id.to_string(), "change": change.id}),
                        Some(&original),
                        None,
                        now,
                    )?;
                }
                agent_intervention(
                    &tx,
                    &change,
                    InterventionKind::Undo,
                    json!({"record": change.record_id.to_string(), "change": change.id}),
                    now,
                )?;
            }
            tx.execute(
                "UPDATE agent_steps SET detail_json = json_set(detail_json, '$.undone', json('true'))
                 WHERE kind = 'RecordChanged' AND json_extract(detail_json, '$.change_id') = ?1",
                [change_id],
            )?;
            let register = load_register(&tx, current.register_id)?;
            let result = match change.before {
                None => {
                    tx.execute("DELETE FROM records WHERE id = ?1", [current.id.to_string()])?;
                    insert_event(
                        &tx,
                        "RecordChangeUndone",
                        &current.id.to_string(),
                        current.revision + 1,
                        json!({"register": register.key, "removed": true}),
                        now,
                    )?;
                    None
                }
                Some(before) => {
                    let revision = current.revision + 1;
                    write_values(&tx, &register, current.id, &before, revision, now)?;
                    tx.execute(
                        "UPDATE record_changes SET undone_at_ms = ?1 WHERE id = ?2",
                        params![now, change_id],
                    )?;
                    insert_change(
                        &tx,
                        current.id,
                        RecordChangeKind::Undo,
                        RecordActor::User,
                        Some(&current.values),
                        &before,
                        revision,
                        None,
                        now,
                    )?;
                    insert_event(
                        &tx,
                        "RecordChangeUndone",
                        &current.id.to_string(),
                        revision,
                        json!({"register": register.key, "removed": false}),
                        now,
                    )?;
                    Some(load_record(&tx, current.id)?)
                }
            };
            tx.commit()?;
            if result.is_none() {
                crate::chat::shrink_journal(conn);
            }
            Ok(result)
        })
        .await
    }

    /// Removes a record with its history; its values are overwritten in the
    /// file (`secure_delete`) and the journal is shrunk.
    pub async fn delete_record(&self, id: RecordId, now: i64) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let current = load_record(&tx, id)?;
            let register = load_register(&tx, current.register_id)?;
            // Deleting what the agent added or wrote corrects it.
            for (change, _) in agent_writers(&tx, id, None)? {
                agent_intervention(
                    &tx,
                    &change,
                    InterventionKind::Correction,
                    json!({"record": id.to_string(), "change": change.id, "deleted": true}),
                    now,
                )?;
            }
            tx.execute("DELETE FROM records WHERE id = ?1", [id.to_string()])?;
            insert_event(
                &tx,
                "RecordDeleted",
                &id.to_string(),
                current.revision + 1,
                json!({"register": register.key}),
                now,
            )?;
            tx.commit()?;
            crate::chat::shrink_journal(conn);
            Ok(())
        })
        .await
    }
}

fn values_json(values: &Values) -> Result<String, StorageError> {
    serde_json::to_string(values).map_err(|e| StorageError::Corrupt(format!("record values: {e}")))
}

fn parse_values(json: &str) -> Result<Values, StorageError> {
    serde_json::from_str(json).map_err(|e| StorageError::Corrupt(format!("record values: {e}")))
}

/// The status and follow-up date, copied out for filtering.
fn derived(register: &RegisterRow, values: &Values) -> (Option<String>, Option<String>) {
    (
        values.get(&register.status_field).cloned(),
        register
            .follow_up_field
            .as_ref()
            .and_then(|f| values.get(f).cloned()),
    )
}

fn write_values(
    tx: &Transaction,
    register: &RegisterRow,
    id: RecordId,
    values: &Values,
    revision: i64,
    now: i64,
) -> Result<(), StorageError> {
    let (status, follow_up) = derived(register, values);
    tx.execute(
        "UPDATE records SET values_json = ?1, status = ?2, follow_up_on = ?3, revision = ?4, updated_at_ms = ?5
         WHERE id = ?6",
        params![values_json(values)?, status, follow_up, revision, now, id.to_string()],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_change(
    tx: &Transaction,
    record: RecordId,
    kind: RecordChangeKind,
    actor: RecordActor,
    before: Option<&Values>,
    after: &Values,
    revision_after: i64,
    origin: Option<&AgentOrigin>,
    now: i64,
) -> Result<i64, StorageError> {
    let origin = origin.filter(|_| actor == RecordActor::Agent);
    let fields: Vec<&String> = after
        .keys()
        .chain(before.into_iter().flat_map(|b| b.keys()))
        .filter(|k| before.and_then(|b| b.get(*k)) != after.get(*k))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    tx.execute(
        "INSERT INTO record_changes (record_id, kind, actor, before_json, after_json, revision_after,
         conversation_id, message_id, read_hosts_json, at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            record.to_string(),
            kind.as_str(),
            actor.as_str(),
            before.map(values_json).transpose()?,
            values_json(after)?,
            revision_after,
            origin.map(|o| o.conversation_id.to_string()),
            origin.map(|o| o.message_id.to_string()),
            serde_json::to_string(&origin.map(|o| o.read_hosts.clone()).unwrap_or_default())
                .unwrap_or_else(|_| "[]".into()),
            now
        ],
    )?;
    let change = tx.last_insert_rowid();
    // The agent's write is an action of its turn (ADR-ACTION-001), done and
    // checked in this very transaction.
    if let Some(origin) = origin
        && let Some(turn) = crate::actions::turn_of_message(tx, &origin.message_id.to_string())?
    {
        crate::actions::record(
            tx,
            crate::Actor::Agent,
            &turn,
            "RecordWrite",
            1,
            EffectClass::InternalReversible,
            Evidence::Verified,
            ActionState::Verified,
            serde_json::json!({
                "record": record.to_string(),
                "change": change,
                "kind": kind.as_str(),
                "fields": fields,
                "revision_after": revision_after,
            }),
            None,
            Some(origin.policy),
            now,
        )?;
    }
    Ok(change)
}

fn load_register(tx: &Transaction, id: RegisterId) -> Result<RegisterRow, StorageError> {
    let raw: Option<(String, String, String, String, Option<String>)> = tx
        .query_row(
            "SELECT key, name, schema_json, status_field, follow_up_field FROM registers WHERE id = ?1",
            [id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    let (key, name, schema_json, status_field, follow_up_field) =
        raw.ok_or(StorageError::NotFound("register"))?;
    Ok(RegisterRow {
        id,
        key,
        name,
        schema_json,
        status_field,
        follow_up_field,
    })
}

struct RawRecord {
    id: String,
    register_id: String,
    values_json: String,
    status: Option<String>,
    follow_up_on: Option<String>,
    revision: i64,
    created_by: String,
    created_at_ms: i64,
    updated_at_ms: i64,
}

fn raw_record(r: &rusqlite::Row) -> rusqlite::Result<RawRecord> {
    Ok(RawRecord {
        id: r.get(0)?,
        register_id: r.get(1)?,
        values_json: r.get(2)?,
        status: r.get(3)?,
        follow_up_on: r.get(4)?,
        revision: r.get(5)?,
        created_by: r.get(6)?,
        created_at_ms: r.get(7)?,
        updated_at_ms: r.get(8)?,
    })
}

impl RawRecord {
    fn parse(self) -> Result<RecordRow, StorageError> {
        Ok(RecordRow {
            id: RecordId(parse_uuid(&self.id)?),
            register_id: RegisterId(parse_uuid(&self.register_id)?),
            values: parse_values(&self.values_json)?,
            status: self.status,
            follow_up_on: self.follow_up_on,
            revision: self.revision,
            created_by: parse_state(&self.created_by)?,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
        })
    }
}

fn load_record(tx: &Transaction, id: RecordId) -> Result<RecordRow, StorageError> {
    tx.query_row(
        &format!("SELECT {RECORD_COLUMNS} FROM records WHERE id = ?1"),
        [id.to_string()],
        raw_record,
    )
    .optional()?
    .ok_or(StorageError::NotFound("record"))?
    .parse()
}

struct RawChange {
    id: i64,
    record_id: String,
    kind: String,
    actor: String,
    before_json: Option<String>,
    after_json: String,
    revision_after: i64,
    conversation_id: Option<String>,
    message_id: Option<String>,
    read_hosts_json: String,
    undone_at_ms: Option<i64>,
    at_ms: i64,
    agent: Option<String>,
}

fn raw_change(r: &rusqlite::Row) -> rusqlite::Result<RawChange> {
    Ok(RawChange {
        id: r.get(0)?,
        record_id: r.get(1)?,
        kind: r.get(2)?,
        actor: r.get(3)?,
        before_json: r.get(4)?,
        after_json: r.get(5)?,
        revision_after: r.get(6)?,
        conversation_id: r.get(7)?,
        message_id: r.get(8)?,
        read_hosts_json: r.get(9)?,
        undone_at_ms: r.get(10)?,
        at_ms: r.get(11)?,
        agent: r.get(12)?,
    })
}

impl RawChange {
    fn parse(self) -> Result<RecordChangeRow, StorageError> {
        Ok(RecordChangeRow {
            id: self.id,
            record_id: RecordId(parse_uuid(&self.record_id)?),
            kind: parse_state(&self.kind)?,
            actor: parse_state(&self.actor)?,
            before: self.before_json.as_deref().map(parse_values).transpose()?,
            after: parse_values(&self.after_json)?,
            revision_after: self.revision_after,
            conversation_id: self
                .conversation_id
                .map(|c| Ok::<_, StorageError>(ConversationId(parse_uuid(&c)?)))
                .transpose()?,
            message_id: self
                .message_id
                .map(|m| Ok::<_, StorageError>(MessageId(parse_uuid(&m)?)))
                .transpose()?,
            read_hosts: serde_json::from_str(&self.read_hosts_json).unwrap_or_default(),
            undone_at_ms: self.undone_at_ms,
            at_ms: self.at_ms,
            agent: self.agent,
        })
    }
}

/// The agent's changes that last wrote the given fields of a record (every
/// field when `fields` is `None`), each with the fields it wrote; undone
/// changes do not count.
fn agent_writers(
    tx: &Transaction,
    record: RecordId,
    fields: Option<&[&String]>,
) -> Result<Vec<(RecordChangeRow, Vec<String>)>, StorageError> {
    let mut stmt = tx.prepare(&format!(
        "SELECT {CHANGE_COLUMNS} FROM record_changes WHERE record_id = ?1 ORDER BY id DESC"
    ))?;
    let history = stmt
        .query_map([record.to_string()], raw_change)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(RawChange::parse)
        .collect::<Result<Vec<_>, _>>()?;
    let keys: Vec<String> = match fields {
        Some(fields) => fields.iter().map(|k| (*k).clone()).collect(),
        None => history
            .iter()
            .flat_map(|c| c.after.keys().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
    };
    let mut out: Vec<(RecordChangeRow, Vec<String>)> = Vec::new();
    for key in keys {
        // The newest change that set this field decides who wrote it.
        let writer = history.iter().find(|c| {
            let before = c.before.as_ref().and_then(|b| b.get(&key));
            c.after.get(&key) != before && (c.after.contains_key(&key) || before.is_some())
        });
        let Some(writer) =
            writer.filter(|c| c.actor == RecordActor::Agent && c.undone_at_ms.is_none())
        else {
            continue;
        };
        match out.iter_mut().find(|(c, _)| c.id == writer.id) {
            Some((_, keys)) => keys.push(key),
            None => out.push((writer.clone(), vec![key])),
        }
    }
    Ok(out)
}

/// Records an intervention on the turn that made an agent's change.
fn agent_intervention(
    tx: &Transaction,
    change: &RecordChangeRow,
    kind: InterventionKind,
    detail: serde_json::Value,
    now: i64,
) -> Result<(), StorageError> {
    let conversation = change.conversation_id.map(|c| c.to_string());
    let message = change.message_id.map(|m| m.to_string());
    if let Some((task, run)) =
        crate::interventions::turn_of(tx, conversation.as_deref(), message.as_deref())?
    {
        crate::interventions::record(
            tx,
            crate::Actor::LocalUser,
            &task,
            run.as_deref(),
            None,
            kind,
            detail,
            now,
        )?;
    }
    Ok(())
}
