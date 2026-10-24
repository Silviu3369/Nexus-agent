//! Automations and their history (`schema::V10`) on a real SQLite file:
//! an edit made elsewhere is not overwritten, only enabled automations
//! whose time has come are due, recording a run moves the schedule on only
//! while it still expects that time, a deleted task leaves its entry
//! without it, deleting an automation keeps its tasks, events never carry
//! the instruction, and the migration keeps every message.

mod common;

use std::path::PathBuf;

use nexus_protocol::{AutomationRunStatus, ConversationId, Schedule};
use nexus_storage::{
    AutomationRunRow, NewAutomation, NewExchange, SUPPORTED_VERSION, Settlement, StorageError,
    Store,
};

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
    let dir = std::env::temp_dir().join(format!("nexus-automation-test-{}", uuid::Uuid::new_v4()));
    let store = Store::open(&dir.join("nexus.db")).unwrap();
    store.persona(0).await.unwrap();
    store.ensure_default_project(0).await.unwrap();
    Env { store, dir }
}

const PROMPT: &str = "SENTINEL-automation-prompt: summarise my unread notes";

fn daily(next: Option<i64>) -> NewAutomation {
    NewAutomation {
        name: "Morning summary".into(),
        prompt: PROMPT.into(),
        project_id: None,
        schedule: Schedule::Weekly {
            days: vec![0, 1, 2, 3, 4, 5, 6],
            hour: 9,
            minute: 0,
        },
        enabled: next.is_some(),
        catch_up: true,
        next_run_at_ms: next,
    }
}

async fn task(store: &Store, key: &str) -> ConversationId {
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
    store
        .append_exchange(
            NewExchange {
                requester: nexus_storage::Requester::User,
                pack: None,
                conversation_id: None,
                project_id: None,
                idempotency_key: key.into(),
                text: PROMPT.into(),
                connection_id: connection,
                model: "m".into(),
                persona_version: 1,
                title: Some("Morning summary · 2026-10-19 09:00".into()),
            },
            5,
        )
        .await
        .unwrap()
        .conversation_id
}

fn started(conversation: ConversationId, at: i64) -> AutomationRunRow {
    AutomationRunRow {
        automation_id: Default::default(),
        scheduled_for_ms: at,
        at_ms: at + 1,
        status: AutomationRunStatus::Started,
        reason: None,
        conversation_id: Some(conversation),
        catch_up: false,
        manual: false,
    }
}

#[tokio::test]
async fn an_edit_made_elsewhere_is_not_overwritten() {
    let env = env().await;
    let s = &env.store;
    let created = s.create_automation(daily(Some(1_000)), 1).await.unwrap();
    assert_eq!(created.revision, 1);
    assert_eq!(created.next_run_at_ms, Some(1_000));

    let mut renamed = daily(Some(2_000));
    renamed.name = "Evening summary".into();
    let edited = s
        .update_automation(created.id, 1, renamed.clone(), 2)
        .await
        .unwrap();
    assert_eq!(
        (edited.revision, edited.name.as_str()),
        (2, "Evening summary")
    );
    assert!(matches!(
        s.update_automation(created.id, 1, renamed, 3).await,
        Err(StorageError::RevisionConflict(_))
    ));
    assert_eq!(s.automations().await.unwrap(), [edited]);
}

#[tokio::test]
async fn only_enabled_automations_whose_time_has_come_are_due() {
    let env = env().await;
    let s = &env.store;
    let soon = s.create_automation(daily(Some(1_000)), 1).await.unwrap();
    s.create_automation(daily(Some(5_000)), 1).await.unwrap();
    s.create_automation(daily(None), 1).await.unwrap();
    let due: Vec<_> = s
        .due_automations(1_000)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(due, [soon.id]);
    // The schema refuses a disabled automation with a next time.
    let refused = s
        .call(|c| {
            Ok(c.execute(
                "UPDATE automations SET enabled = 0 WHERE next_run_at_ms IS NOT NULL",
                [],
            )?)
        })
        .await;
    assert!(refused.is_err());
}

#[tokio::test]
async fn recording_a_run_moves_on_only_while_that_time_is_still_expected() {
    let env = env().await;
    let s = &env.store;
    let a = s.create_automation(daily(Some(1_000)), 1).await.unwrap();
    let conversation = task(s, &format!("auto:{}:1000", a.id)).await;
    assert_eq!(
        s.conversation_with_key(format!("auto:{}:1000", a.id))
            .await
            .unwrap(),
        Some(conversation)
    );
    assert!(s.conversation_answering(conversation).await.unwrap());

    assert!(
        s.record_automation_runs(
            a.id,
            Some(1_000),
            vec![started(conversation, 1_000)],
            Some(2_000),
            1_001
        )
        .await
        .unwrap()
    );
    assert_eq!(
        s.automation(a.id).await.unwrap().next_run_at_ms,
        Some(2_000)
    );

    // Edited meanwhile: the run is recorded, the edit's next time stays.
    s.record_automation_runs(
        a.id,
        Some(1_000),
        vec![AutomationRunRow {
            status: AutomationRunStatus::Skipped,
            reason: Some("x".repeat(600)),
            conversation_id: None,
            ..started(conversation, 1_500)
        }],
        Some(9_999),
        1_501,
    )
    .await
    .unwrap();
    assert_eq!(
        s.automation(a.id).await.unwrap().next_run_at_ms,
        Some(2_000)
    );
    // A manual run never moves the schedule.
    s.record_automation_runs(
        a.id,
        None,
        vec![AutomationRunRow {
            manual: true,
            ..started(conversation, 1_600)
        }],
        None,
        1_601,
    )
    .await
    .unwrap();
    assert_eq!(
        s.automation(a.id).await.unwrap().next_run_at_ms,
        Some(2_000)
    );

    let runs = s.automation_runs(a.id, 10).await.unwrap();
    assert_eq!(
        runs.iter().map(|r| r.scheduled_for_ms).collect::<Vec<_>>(),
        [1_600, 1_500, 1_000],
        "newest first"
    );
    assert!(runs[0].manual);
    assert_eq!(runs[1].reason.as_ref().unwrap().chars().count(), 500);
    assert_eq!(runs[2].conversation_id, Some(conversation));
    assert_eq!(s.automation_runs(a.id, 1).await.unwrap().len(), 1);

    s.delete_automation(a.id, 2).await.unwrap();
    assert!(
        !s.record_automation_runs(
            a.id,
            Some(2_000),
            vec![started(conversation, 2_000)],
            None,
            3
        )
        .await
        .unwrap(),
        "nothing is recorded for a deleted automation"
    );
}

#[tokio::test]
async fn deleting_a_task_or_an_automation_keeps_the_other() {
    let env = env().await;
    let s = &env.store;
    let a = s.create_automation(daily(Some(1_000)), 1).await.unwrap();
    let first = task(s, "auto:first").await;
    let second = task(s, "auto:second").await;
    s.record_automation_runs(
        a.id,
        Some(1_000),
        vec![started(first, 1_000), started(second, 1_500)],
        Some(2_000),
        2,
    )
    .await
    .unwrap();

    // A task whose reply is still being written cannot be deleted.
    let (_, messages) = s.messages(first, 10).await.unwrap();
    s.settle_message(
        messages[1].id,
        nexus_core::MessageStatus::Complete,
        Settlement::default(),
        3,
    )
    .await
    .unwrap();
    s.delete_conversation(first, 3).await.unwrap();
    let runs = s.automation_runs(a.id, 10).await.unwrap();
    assert_eq!(runs.len(), 2, "the entry stays without its task");
    assert_eq!(runs[1].conversation_id, None);
    assert_eq!(runs[0].conversation_id, Some(second));

    s.delete_automation(a.id, 4).await.unwrap();
    assert!(matches!(
        s.automation(a.id).await,
        Err(StorageError::NotFound("automation"))
    ));
    assert!(s.automations().await.unwrap().is_empty());
    let (runs_left, tasks_left): (i64, i64) = s
        .call(|c| {
            Ok(c.query_row(
                "SELECT (SELECT count(*) FROM automation_runs), (SELECT count(*) FROM conversations)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!((runs_left, tasks_left), (0, 1));

    let events = s.events_since(0, 500).await.unwrap();
    let kinds: Vec<_> = events
        .iter()
        .filter(|e| e.kind.starts_with("Automation"))
        .map(|e| e.kind.as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "AutomationCreated",
            "AutomationRan",
            "AutomationRan",
            "AutomationDeleted"
        ]
    );
    assert!(
        events
            .iter()
            .filter(|e| e.kind.starts_with("Automation"))
            .all(|e| !e.payload_json.contains("SENTINEL")),
        "events never carry the instruction"
    );
}

#[tokio::test]
async fn migrating_to_v10_keeps_every_message() {
    let dir = std::env::temp_dir().join(format!("nexus-migrate-v10-test-{}", uuid::Uuid::new_v4()));
    let path = dir.join("nexus.db");
    let conversation = {
        let store = Store::open(&path).unwrap();
        store.persona(0).await.unwrap();
        store.ensure_default_project(0).await.unwrap();
        let conversation = task(&store, "m1").await;
        store
            .call(|c| {
                common::undo_v15(c)?;
                c.execute_batch(
                    "DROP TABLE inbox_files; DROP TABLE register_inboxes; DROP TABLE folders; DROP TABLE record_changes; DROP TABLE records; DROP TABLE registers; DROP TABLE automation_runs; DROP TABLE automations; PRAGMA user_version = 9;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        conversation
    };
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let (row, messages) = store.messages(conversation, 10).await.unwrap();
    assert_eq!(
        row.title, "Morning summary · 2026-10-19 09:00",
        "the title given"
    );
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].text, PROMPT);
    assert!(store.automations().await.unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
