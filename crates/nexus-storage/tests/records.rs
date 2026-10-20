//! Registers and records (`schema::V11`) on a real SQLite file: a register
//! is created once and kept up to date, filters find what they say, an
//! edit made elsewhere is not overwritten, every change has its history,
//! only a record's latest change is undone (and the agent's step says so),
//! an agent's change names its task until that task is deleted, deleting a
//! record removes its history, events never carry values, and the
//! migration keeps every message and step.

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use nexus_core::{AgentStepKind, AgentStepStatus, RecordActor, RecordChangeKind};
use nexus_protocol::{ConversationId, MessageId, RegisterId};
use nexus_storage::{
    AgentOrigin, NewExchange, RecordFilter, SUPPORTED_VERSION, StorageError, Store, Values,
};
use serde_json::json;

struct Env {
    store: Store,
    dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn env() -> Env {
    let dir = std::env::temp_dir().join(format!("nexus-records-test-{}", uuid::Uuid::new_v4()));
    let store = Store::open(&dir.join("nexus.db")).unwrap();
    store.persona(0).await.unwrap();
    store.ensure_default_project(0).await.unwrap();
    Env { store, dir }
}

async fn jobs(store: &Store) -> RegisterId {
    store
        .ensure_register(
            "job_applications".into(),
            "Job applications".into(),
            r#"{"version":1}"#.into(),
            "status".into(),
            Some("follow_up_on".into()),
            1,
        )
        .await
        .unwrap()
        .id
}

fn values(pairs: &[(&str, &str)]) -> Values {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect::<BTreeMap<_, _>>()
}

const SECRETISH: &str = "SENTINEL-record-value";

/// A task with a reply, as the agent's changes come from one.
async fn task(store: &Store) -> (ConversationId, MessageId) {
    let connection = store
        .save_connection(
            None,
            "Gemini",
            "g".into(),
            false,
            format!("gemini-{}", uuid::Uuid::new_v4()),
            1,
        )
        .await
        .unwrap()
        .id;
    let ex = store
        .append_exchange(
            NewExchange {
                origin: nexus_core::TaskOrigin::User,
                pack: None,
                conversation_id: None,
                project_id: None,
                idempotency_key: uuid::Uuid::new_v4().to_string(),
                text: "I applied at Acme".into(),
                connection_id: connection,
                model: "m".into(),
                persona_version: 1,
                title: None,
            },
            2,
        )
        .await
        .unwrap();
    (ex.conversation_id, ex.model_message.id)
}

#[tokio::test]
async fn a_register_is_created_once_and_kept_up_to_date() {
    let env = env().await;
    let s = &env.store;
    let first = jobs(s).await;
    assert_eq!(jobs(s).await, first, "the same register, not a second one");
    let renamed = s
        .ensure_register(
            "job_applications".into(),
            "Jobs".into(),
            r#"{"version":2}"#.into(),
            "status".into(),
            None,
            2,
        )
        .await
        .unwrap();
    assert_eq!(renamed.id, first);
    assert_eq!(
        (renamed.name.as_str(), renamed.schema_json.as_str()),
        ("Jobs", r#"{"version":2}"#)
    );
    assert_eq!(renamed.follow_up_field, None);
    assert_eq!(s.registers().await.unwrap().len(), 1);
}

#[tokio::test]
async fn filters_find_what_they_say() {
    let env = env().await;
    let s = &env.store;
    let reg = jobs(s).await;
    let add = |v: Values| s.add_record(reg, v, RecordActor::User, None, 5);
    add(values(&[
        ("company", "Acme"),
        ("role", "Service Desk"),
        ("status", "Applied"),
        ("applied_on", "2026-10-01"),
        ("follow_up_on", "2026-10-15"),
    ]))
    .await
    .unwrap();
    add(values(&[
        ("company", "Beta 100%"),
        ("role", "IT Support"),
        ("status", "Rejected"),
        ("applied_on", "2026-09-20"),
        ("follow_up_on", "2026-10-01"),
    ]))
    .await
    .unwrap();
    add(values(&[
        ("company", "Gamma"),
        ("role", "Sysadmin"),
        ("status", "To apply"),
        ("follow_up_on", "2026-11-01"),
    ]))
    .await
    .unwrap();

    let names = |rows: Vec<nexus_storage::RecordRow>| -> Vec<String> {
        let mut n: Vec<String> = rows
            .into_iter()
            .map(|r| r.values["company"].clone())
            .collect();
        n.sort();
        n
    };
    let find = |f: RecordFilter| s.records(reg, RecordFilter { limit: 50, ..f });
    let (rows, total) = find(RecordFilter::default()).await.unwrap();
    assert_eq!((rows.len(), total), (3, 3));

    let (rows, _) = find(RecordFilter {
        statuses: vec!["Applied".into(), "To apply".into()],
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(names(rows), ["Acme", "Gamma"]);

    let (rows, _) = find(RecordFilter {
        text: Some("acme".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(names(rows), ["Acme"], "text ignores case");
    let (rows, _) = find(RecordFilter {
        text: Some("100%".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(names(rows), ["Beta 100%"], "% is matched literally");
    let (rows, _) = find(RecordFilter {
        text: Some("%".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(names(rows), ["Beta 100%"]);

    // Due by 18 October: Acme (15th); Beta is closed; Gamma is later.
    let (rows, _) = find(RecordFilter {
        follow_up_by: Some("2026-10-18".into()),
        closed_statuses: vec!["Rejected".into(), "Withdrawn".into()],
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(names(rows), ["Acme"]);
    assert_eq!(
        s.register_counts(reg, "2026-10-18".into(), vec!["Rejected".into()])
            .await
            .unwrap(),
        (3, 1)
    );

    let (rows, _) = find(RecordFilter {
        date_range: Some((
            "applied_on".into(),
            Some("2026-09-25".into()),
            Some("2026-10-31".into()),
        )),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(names(rows), ["Acme"]);

    let (rows, total) = s
        .records(
            reg,
            RecordFilter {
                limit: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (rows.len(), total),
        (1, 3),
        "the count is of all that match"
    );
}

#[tokio::test]
async fn every_change_has_history_and_only_the_latest_is_undone() {
    let env = env().await;
    let s = &env.store;
    let reg = jobs(s).await;
    let (conversation, reply) = task(s).await;
    let origin = AgentOrigin {
        conversation_id: conversation,
        message_id: reply,
        read_hosts: vec!["jobs.example.com".into()],
    };
    let added = s
        .add_record(
            reg,
            values(&[
                ("company", "Acme"),
                ("role", "Service Desk"),
                ("status", "To apply"),
            ]),
            RecordActor::Agent,
            Some(origin.clone()),
            10,
        )
        .await
        .unwrap();
    let id = added.record.id;
    assert_eq!(added.record.created_by, RecordActor::Agent);
    assert_eq!(added.record.status.as_deref(), Some("To apply"));

    // The user's stale edit is refused; the same values change nothing.
    let applied = values(&[
        ("company", "Acme"),
        ("role", "Service Desk"),
        ("status", "Applied"),
        ("follow_up_on", "2026-10-25"),
    ]);
    assert!(matches!(
        s.update_record(id, Some(9), applied.clone(), RecordActor::User, None, 11)
            .await,
        Err(StorageError::RevisionConflict(_))
    ));
    let updated = s
        .update_record(
            id,
            None,
            applied.clone(),
            RecordActor::Agent,
            Some(origin),
            12,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.record.revision, 2);
    assert_eq!(updated.record.follow_up_on.as_deref(), Some("2026-10-25"));
    assert!(
        s.update_record(id, None, applied, RecordActor::Agent, None, 13)
            .await
            .unwrap()
            .is_none()
    );

    let history = s.record_history(id).await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].kind, RecordChangeKind::Updated);
    assert_eq!(history[0].before.as_ref().unwrap()["status"], "To apply");
    assert_eq!(history[0].conversation_id, Some(conversation));
    assert_eq!(history[0].read_hosts, ["jobs.example.com"]);

    // The agent's step for the update is marked undone with it.
    let step = s
        .begin_step(
            reply,
            AgentStepKind::RecordChanged,
            json!({"change_id": updated.change_id, "undone": false}),
            14,
        )
        .await
        .unwrap();
    s.finish_step(
        step.id,
        AgentStepStatus::Done,
        json!({"change_id": updated.change_id, "undone": false}),
        14,
    )
    .await
    .unwrap();

    assert!(
        matches!(
            s.undo_record_change(added.change_id, 15).await,
            Err(StorageError::NotUndoable(_))
        ),
        "an earlier change waits for the later one"
    );
    let restored = s
        .undo_record_change(updated.change_id, 16)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.values["status"], "To apply");
    assert_eq!(restored.follow_up_on, None);
    assert_eq!(restored.revision, 3);
    let steps = s.steps_for_conversation(conversation).await.unwrap();
    assert_eq!(steps[0].detail["undone"], json!(true));
    assert!(
        matches!(
            s.undo_record_change(updated.change_id, 17).await,
            Err(StorageError::NotUndoable(_))
        ),
        "an undone change is not undone twice"
    );
    let history = s.record_history(id).await.unwrap();
    assert_eq!(history[0].kind, RecordChangeKind::Undo);
    assert!(matches!(
        s.undo_record_change(history[0].id, 18).await,
        Err(StorageError::NotUndoable(_))
    ));
    assert!(history[1].undone_at_ms.is_some());

    // The addition is undone only from where it started: a newer edit first.
    s.update_record(
        id,
        Some(3),
        values(&[
            ("company", "Acme"),
            ("role", "Service Desk"),
            ("status", "To apply"),
            ("notes", "x"),
        ]),
        RecordActor::User,
        None,
        19,
    )
    .await
    .unwrap();
    assert!(s.undo_record_change(added.change_id, 20).await.is_err());
    let fresh = s
        .add_record(
            reg,
            values(&[("company", "Beta"), ("role", "IT"), ("status", "To apply")]),
            RecordActor::Agent,
            None,
            21,
        )
        .await
        .unwrap();
    assert_eq!(
        s.undo_record_change(fresh.change_id, 22).await.unwrap(),
        None
    );
    assert!(matches!(
        s.record(fresh.record.id).await,
        Err(StorageError::NotFound("record"))
    ));
}

#[tokio::test]
async fn a_deleted_task_leaves_the_change_without_it_and_a_deleted_record_takes_its_history() {
    let env = env().await;
    let s = &env.store;
    let reg = jobs(s).await;
    let (conversation, reply) = task(s).await;
    s.settle_message(
        reply,
        nexus_core::MessageStatus::Complete,
        Default::default(),
        3,
    )
    .await
    .unwrap();
    let added = s
        .add_record(
            reg,
            values(&[
                ("company", "Acme"),
                ("role", "Service Desk"),
                ("status", "To apply"),
                ("notes", SECRETISH),
            ]),
            RecordActor::Agent,
            Some(AgentOrigin {
                conversation_id: conversation,
                message_id: reply,
                read_hosts: vec![],
            }),
            4,
        )
        .await
        .unwrap();
    s.delete_conversation(conversation, 5).await.unwrap();
    let history = s.record_history(added.record.id).await.unwrap();
    assert_eq!(history[0].conversation_id, None);
    assert_eq!(history[0].message_id, None);
    assert_eq!(history[0].actor, RecordActor::Agent);

    // A user's change cannot name a task.
    let (other, _) = task(s).await;
    let refused = s
        .call(move |c| {
            Ok(c.execute(
                "UPDATE record_changes SET actor = 'User', conversation_id = ?1",
                [other.to_string()],
            )?)
        })
        .await;
    assert!(refused.is_err());

    s.delete_record(added.record.id, 6).await.unwrap();
    assert!(matches!(
        s.record_history(added.record.id).await,
        Err(StorageError::NotFound(_))
    ));
    let left: i64 = s
        .call(|c| Ok(c.query_row("SELECT count(*) FROM record_changes", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(left, 0);

    let events = s.events_since(0, 500).await.unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .filter(|e| e.kind.starts_with("Record"))
        .map(|e| e.kind.as_str())
        .collect();
    assert_eq!(kinds, ["RecordAdded", "RecordDeleted"]);
    assert!(
        events.iter().all(|e| !e.payload_json.contains(SECRETISH)),
        "events never carry values"
    );
}

#[tokio::test]
async fn migrating_to_v11_keeps_every_message_and_step() {
    let dir = std::env::temp_dir().join(format!("nexus-migrate-v11-test-{}", uuid::Uuid::new_v4()));
    let path = dir.join("nexus.db");
    let conversation = {
        let store = Store::open(&path).unwrap();
        store.persona(0).await.unwrap();
        store.ensure_default_project(0).await.unwrap();
        let (conversation, reply) = task(&store).await;
        store
            .begin_step(
                reply,
                AgentStepKind::Fetch,
                json!({"url": "https://example.com"}),
                3,
            )
            .await
            .unwrap();
        store
            .call(|c| {
                common::undo_v15(c)?;
                c.execute_batch(
                    "PRAGMA foreign_keys = OFF;
                     DROP TABLE inbox_files; DROP TABLE register_inboxes; DROP TABLE folders; DROP TABLE record_changes; DROP TABLE records; DROP TABLE registers;
                     CREATE TABLE agent_steps_v10 (
                         id TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages (id),
                         seq INTEGER NOT NULL CHECK (seq >= 1),
                         kind TEXT NOT NULL CHECK (kind IN ('ModelCall', 'Fetch', 'NoteProposed', 'MemoryProposed', 'McpCall')),
                         status TEXT NOT NULL CHECK (status IN ('Running', 'Done', 'Failed')),
                         detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
                         started_at_ms INTEGER NOT NULL, finished_at_ms INTEGER,
                         CHECK ((status = 'Running') = (finished_at_ms IS NULL)), UNIQUE (message_id, seq)) STRICT;
                     INSERT INTO agent_steps_v10 SELECT * FROM agent_steps;
                     DROP TABLE agent_steps;
                     ALTER TABLE agent_steps_v10 RENAME TO agent_steps;
                     CREATE INDEX agent_steps_by_start ON agent_steps (started_at_ms);
                     CREATE INDEX agent_steps_by_status ON agent_steps (status);
                     PRAGMA user_version = 10;
                     PRAGMA foreign_keys = ON;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        conversation
    };
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let (_, messages) = store.messages(conversation, 10).await.unwrap();
    assert_eq!(messages.len(), 2);
    let steps = store.steps_for_conversation(conversation).await.unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, AgentStepKind::Fetch);
    // The new step kinds are accepted.
    store
        .begin_step(messages[1].id, AgentStepKind::RecordChanged, json!({}), 4)
        .await
        .unwrap();
    assert!(store.registers().await.unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
