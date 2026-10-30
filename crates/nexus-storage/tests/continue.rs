//! Replies waiting on the user's decision (`schema::V8`) on a real SQLite
//! file: the waiting state is written with the reply's settlement, the task
//! continues once (a second caller gets nothing), a new request or a
//! deletion drops it, and the migration keeps every message.

mod common;

use std::path::PathBuf;

use nexus_core::MessageStatus;
use nexus_protocol::{ConversationId, RunId};
use nexus_storage::{
    Exchange, NewExchange, Pause, SUPPORTED_VERSION, Settlement, Store, SubmitOutcome,
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

fn env() -> Env {
    let dir = std::env::temp_dir().join(format!("nexus-continue-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

async fn exchange(store: &Store, conversation: Option<ConversationId>, key: &str) -> Exchange {
    store.persona(0).await.unwrap();
    store.ensure_default_project(0).await.unwrap();
    let connection = match store.connections().await.unwrap().first() {
        Some(c) => c.id,
        None => {
            store
                .save_connection(
                    None,
                    "Gemini",
                    "g".into(),
                    false,
                    "gemini-continue".into(),
                    1,
                )
                .await
                .unwrap()
                .id
        }
    };
    store
        .append_exchange(
            NewExchange {
                requester: nexus_storage::Requester::User,
                pack: None,
                agent: None,
                conversation_id: conversation,
                project_id: None,
                idempotency_key: key.into(),
                text: format!("request {key}"),
                connection_id: connection,
                model: "gemini-test".into(),
                persona_version: 1,
                title: None,
            },
            2,
        )
        .await
        .unwrap()
}

async fn new_run(store: &Store, key: &str) -> RunId {
    let project = store.ensure_default_project(0).await.unwrap();
    match store
        .submit_task(
            nexus_storage::EffectRequester::Agent,
            project,
            key.into(),
            "{}".into(),
            key.into(),
            3,
        )
        .await
        .unwrap()
    {
        SubmitOutcome::Created { run_id, .. } | SubmitOutcome::Existing { run_id, .. } => run_id,
    }
}

/// Settles the reply as stopped on `run`, waiting for the user.
async fn pause(store: &Store, ex: &Exchange, run: RunId) {
    store
        .settle_message(
            ex.model_message.id,
            MessageStatus::Complete,
            Settlement {
                proposed_run_id: Some(run),
                pause: Some(Pause {
                    run_id: run,
                    state_json: r#"{"version":1}"#.into(),
                }),
                ..Settlement::default()
            },
            4,
        )
        .await
        .unwrap();
}

async fn set_run_state(store: &Store, run: RunId, state: &'static str) {
    store
        .call(move |c| {
            c.execute(
                "UPDATE runs SET state = ?1 WHERE id = ?2",
                [state, &run.to_string()],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

async fn waiting_rows(store: &Store) -> i64 {
    store
        .call(|c| Ok(c.query_row("SELECT count(*) FROM paused_turns", [], |r| r.get(0))?))
        .await
        .unwrap()
}

#[tokio::test]
async fn a_waiting_reply_continues_once_after_the_decision() {
    let env = env();
    let ex = exchange(&env.store, None, "w1").await;
    let run = new_run(&env.store, "w1-run").await;
    pause(&env.store, &ex, run).await;
    assert_eq!(waiting_rows(&env.store).await, 1);

    assert!(env.store.paused_ready().await.unwrap().is_empty());
    assert!(
        env.store
            .begin_continuation(ex.model_message.id, 5)
            .await
            .unwrap()
            .is_none(),
        "nothing continues before the user decides"
    );

    set_run_state(&env.store, run, "Denied").await;
    assert_eq!(
        env.store.paused_ready().await.unwrap(),
        [ex.model_message.id]
    );
    let c = env
        .store
        .begin_continuation(ex.model_message.id, 6)
        .await
        .unwrap()
        .expect("it continues");
    assert_eq!(c.run_id, run);
    assert_eq!(c.request.id, ex.user_message.id);
    assert_eq!(c.paused_reply.id, ex.model_message.id);
    assert_eq!(c.model_message.seq, ex.model_message.seq + 1);
    assert_eq!(c.model_message.status, MessageStatus::Streaming);
    assert_eq!(c.model_message.model.as_deref(), Some("gemini-test"));
    assert_eq!(c.state_json, r#"{"version":1}"#);

    assert!(
        env.store
            .begin_continuation(ex.model_message.id, 7)
            .await
            .unwrap()
            .is_none(),
        "a second caller gets nothing"
    );
    assert!(env.store.paused_ready().await.unwrap().is_empty());
    assert_eq!(
        waiting_rows(&env.store).await,
        0,
        "the waiting state is consumed"
    );
    let events = env.store.events_since(0, 1000).await.unwrap();
    assert!(events.iter().any(|e| e.kind == "MessageContinued"));
}

#[tokio::test]
async fn left_for_review_counts_as_decided_and_a_failed_reply_keeps_nothing() {
    let env = env();
    let ex = exchange(&env.store, None, "f1").await;
    let run = new_run(&env.store, "f1-run").await;
    pause(&env.store, &ex, run).await;
    set_run_state(&env.store, run, "NeedsReview").await;
    assert_eq!(env.store.paused_ready().await.unwrap().len(), 1);

    let failed = exchange(&env.store, None, "f2").await;
    env.store
        .settle_message(
            failed.model_message.id,
            MessageStatus::Failed,
            Settlement {
                pause: Some(Pause {
                    run_id: run,
                    state_json: "{}".into(),
                }),
                ..Settlement::default()
            },
            4,
        )
        .await
        .unwrap();
    assert_eq!(
        waiting_rows(&env.store).await,
        1,
        "only a completed reply waits"
    );
}

#[tokio::test]
async fn a_new_request_or_a_deletion_drops_the_waiting_state() {
    let env = env();
    let ex = exchange(&env.store, None, "d1").await;
    let run = new_run(&env.store, "d1-run").await;
    pause(&env.store, &ex, run).await;
    let next = exchange(&env.store, Some(ex.conversation_id), "d2").await;
    assert_eq!(waiting_rows(&env.store).await, 0, "the user moved on");
    set_run_state(&env.store, run, "Succeeded").await;
    assert!(env.store.paused_ready().await.unwrap().is_empty());

    env.store
        .settle_message(
            next.model_message.id,
            MessageStatus::Complete,
            Settlement::default(),
            5,
        )
        .await
        .unwrap();
    let other = exchange(&env.store, None, "d3").await;
    let run2 = new_run(&env.store, "d3-run").await;
    pause(&env.store, &other, run2).await;
    env.store
        .delete_conversation(other.conversation_id, 6)
        .await
        .unwrap();
    assert_eq!(waiting_rows(&env.store).await, 0, "it goes with its task");
}

#[tokio::test]
async fn migrating_to_v8_keeps_every_message() {
    let dir = std::env::temp_dir().join(format!("nexus-migrate-v8-test-{}", uuid::Uuid::new_v4()));
    let path = dir.join("nexus.db");
    let conversation = {
        let store = Store::open(&path).unwrap();
        let ex = exchange(&store, None, "m1").await;
        store
            .call(|c| {
                common::undo_v15(c)?;
                c.execute_batch(
                    "DROP TABLE inbox_files; DROP TABLE register_inboxes; DROP TABLE folders; DROP TABLE record_changes; DROP TABLE records; DROP TABLE registers; DROP TABLE automation_runs; DROP TABLE automations; DROP TABLE model_usage; DROP TABLE model_prices; DROP TABLE paused_turns; ALTER TABLE runs DROP COLUMN outcome_json;
                     PRAGMA user_version = 7;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        ex.conversation_id
    };
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let (_, messages) = store.messages(conversation, 10).await.unwrap();
    assert_eq!(messages.len(), 2);
    assert!(store.paused_ready().await.unwrap().is_empty());
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}
