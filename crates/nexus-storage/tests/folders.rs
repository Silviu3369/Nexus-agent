//! Shared folders (`schema::V12`) on a real SQLite file: a name or a place
//! is shared once, a stale edit is refused, unsharing removes only the row,
//! events never carry the path, and the migration keeps every step.

mod common;

use std::path::PathBuf;

use nexus_core::AgentStepKind;
use nexus_storage::{NewExchange, SUPPORTED_VERSION, StorageError, Store};
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

fn env() -> Env {
    let dir = std::env::temp_dir().join(format!("nexus-folders-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

const PATH: &str = r"C:\Users\someone\Documents\SENTINEL-Jobs";

#[tokio::test]
async fn a_name_or_a_place_is_shared_once_and_a_stale_edit_is_refused() {
    let env = env();
    let s = &env.store;
    let jobs = s
        .add_folder("Jobs".into(), PATH.into(), false, 1)
        .await
        .unwrap();
    assert_eq!((jobs.revision, jobs.can_write), (1, false));
    for (label, path) in [("jobs", r"C:\Other"), ("Other", &PATH.to_lowercase()[..])] {
        assert!(matches!(
            s.add_folder(label.into(), path.into(), false, 2).await,
            Err(StorageError::AlreadyShared)
        ));
    }
    let writable = s
        .update_folder(jobs.id, 1, "Jobs".into(), PATH.into(), true, 3)
        .await
        .unwrap();
    assert_eq!((writable.revision, writable.can_write), (2, true));
    assert!(matches!(
        s.update_folder(jobs.id, 1, "Jobs".into(), PATH.into(), false, 4)
            .await,
        Err(StorageError::RevisionConflict(_))
    ));
    let other = s
        .add_folder("Letters".into(), r"C:\Letters".into(), true, 5)
        .await
        .unwrap();
    assert!(matches!(
        s.update_folder(other.id, 1, "JOBS".into(), r"C:\Letters".into(), true, 6)
            .await,
        Err(StorageError::AlreadyShared)
    ));
    s.remove_folder(jobs.id, 7).await.unwrap();
    assert_eq!(s.folders().await.unwrap(), [other]);
    let events = s.events_since(0, 100).await.unwrap();
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "FolderShared",
            "FolderUpdated",
            "FolderShared",
            "FolderUnshared"
        ]
    );
    assert!(
        events.iter().all(|e| !e.payload_json.contains("SENTINEL")),
        "events never carry the path"
    );
}

#[tokio::test]
async fn migrating_to_v12_keeps_every_step_and_accepts_the_new_ones() {
    let env = env();
    let s = &env.store;
    s.persona(0).await.unwrap();
    s.ensure_default_project(0).await.unwrap();
    let connection = s
        .save_connection(None, "Gemini", "g".into(), false, "gemini-v12".into(), 1)
        .await
        .unwrap()
        .id;
    let ex = s
        .append_exchange(
            NewExchange {
                requester: nexus_storage::Requester::User,
                pack: None,
                conversation_id: None,
                project_id: None,
                idempotency_key: "m".into(),
                text: "hi".into(),
                connection_id: connection,
                model: "m".into(),
                persona_version: 1,
                title: None,
            },
            2,
        )
        .await
        .unwrap();
    s.begin_step(
        ex.model_message.id,
        AgentStepKind::RecordChanged,
        json!({}),
        3,
    )
    .await
    .unwrap();
    s.call(|c| {
        common::undo_v15(c)?;
        c.execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE inbox_files; DROP TABLE register_inboxes; DROP TABLE folders;
             CREATE TABLE agent_steps_v11 (
                 id TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages (id),
                 seq INTEGER NOT NULL CHECK (seq >= 1),
                 kind TEXT NOT NULL CHECK (kind IN ('ModelCall', 'Fetch', 'NoteProposed', 'MemoryProposed', 'McpCall',
                                                    'RecordsFound', 'RecordChanged')),
                 status TEXT NOT NULL CHECK (status IN ('Running', 'Done', 'Failed')),
                 detail_json TEXT NOT NULL CHECK (json_valid(detail_json)),
                 started_at_ms INTEGER NOT NULL, finished_at_ms INTEGER,
                 CHECK ((status = 'Running') = (finished_at_ms IS NULL)), UNIQUE (message_id, seq)) STRICT;
             INSERT INTO agent_steps_v11 SELECT * FROM agent_steps;
             DROP TABLE agent_steps;
             ALTER TABLE agent_steps_v11 RENAME TO agent_steps;
             CREATE INDEX agent_steps_by_start ON agent_steps (started_at_ms);
             CREATE INDEX agent_steps_by_status ON agent_steps (status);
             PRAGMA user_version = 11;
             PRAGMA foreign_keys = ON;",
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let path = env.dir.join("nexus.db");
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let steps = reopened
        .steps_for_conversation(ex.conversation_id)
        .await
        .unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, AgentStepKind::RecordChanged);
    for kind in [
        AgentStepKind::FilesListed,
        AgentStepKind::DocumentRead,
        AgentStepKind::DocumentProposed,
    ] {
        reopened
            .begin_step(ex.model_message.id, kind, json!({}), 4)
            .await
            .unwrap();
    }
    assert!(reopened.folders().await.unwrap().is_empty());
}
