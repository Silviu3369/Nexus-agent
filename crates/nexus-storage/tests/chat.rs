//! M2 persistence on a real SQLite file: schema migration, connections
//! without secrets, versioned persona and conversations.

use std::path::PathBuf;

use nexus_core::{MessageRole, MessageStatus, ThinkingLevel};
use nexus_protocol::ConnectionId;
use nexus_storage::{NewExchange, SUPPORTED_VERSION, Settlement, StorageError, Store};

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
    let dir = std::env::temp_dir().join(format!("nexus-chat-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

/// A connection plus persona v1 (messages reference both by foreign key).
async fn connection(env: &Env) -> ConnectionId {
    env.store.persona(0).await.unwrap();
    env.store
        .save_connection(
            None,
            "Gemini personal".into(),
            true,
            format!("gemini-{}", uuid::Uuid::new_v4()),
            1,
        )
        .await
        .unwrap()
        .id
}

fn exchange(
    conversation: Option<nexus_protocol::ConversationId>,
    key: &str,
    text: &str,
    connection: ConnectionId,
) -> NewExchange {
    NewExchange {
        conversation_id: conversation,
        idempotency_key: key.to_string(),
        text: text.to_string(),
        connection_id: connection,
        model: "gemini-test".to_string(),
        persona_version: 1,
    }
}

#[tokio::test]
async fn an_existing_database_is_migrated_forward_keeping_its_data() {
    let dir = std::env::temp_dir().join(format!("nexus-migrate-test-{}", uuid::Uuid::new_v4()));
    let path = dir.join("nexus.db");
    {
        let store = Store::open(&path).unwrap();
        let project = store.ensure_default_project(1).await.unwrap();
        store
            .submit_task(project, "k".into(), "{}".into(), "h".into(), 2)
            .await
            .unwrap();
        // Roll the file back to version 1 as an M1 host would have left it.
        store
            .call(|c| {
                c.execute_batch(
                    "DROP TABLE messages; DROP TABLE conversations; DROP TABLE personas;
                     DROP TABLE provider_connections; DROP TABLE settings; PRAGMA user_version = 1;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let (runs, _) = store.snapshot(10).await.unwrap();
    assert_eq!(runs.len(), 1, "M1 data must survive the migration");
    assert_eq!(
        store.persona(3).await.unwrap().version,
        1,
        "new tables must exist"
    );
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn an_existing_connection_defaults_to_low_thinking_after_migrating_to_v3() {
    let dir = std::env::temp_dir().join(format!("nexus-migrate-v3-test-{}", uuid::Uuid::new_v4()));
    let path = dir.join("nexus.db");
    let id = {
        let store = Store::open(&path).unwrap();
        store.persona(0).await.unwrap();
        let id = store
            .save_connection(
                None,
                "Gemini personal".into(),
                true,
                "gemini-v2-test".into(),
                1,
            )
            .await
            .unwrap()
            .id;
        // Roll back to v2, as an M2 host before this change would have left it.
        store
            .call(|c| {
                c.execute_batch("ALTER TABLE provider_connections DROP COLUMN thinking_level; PRAGMA user_version = 2;")?;
                Ok(())
            })
            .await
            .unwrap();
        id
    };

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let row = store.connection(id).await.unwrap();
    assert_eq!(
        row.thinking_level,
        ThinkingLevel::Low,
        "a connection that predates the column keeps the behaviour it already had"
    );

    let updated = store
        .set_thinking_level(id, ThinkingLevel::High, 10)
        .await
        .unwrap();
    assert_eq!(updated.thinking_level, ThinkingLevel::High);
    assert_eq!(
        store.connection(id).await.unwrap().thinking_level,
        ThinkingLevel::High
    );

    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn connections_hold_only_a_vault_reference_and_active_selection_is_explicit() {
    let env = env();
    assert!(
        env.store.active_connection().await.unwrap().is_none(),
        "no implicit default provider"
    );
    let id = connection(&env).await;
    assert_eq!(
        env.store.connection(id).await.unwrap().thinking_level,
        ThinkingLevel::Low,
        "a new connection starts at the fast default"
    );

    let columns: Vec<String> = env
        .store
        .call(|c| {
            let mut stmt =
                c.prepare("SELECT name FROM pragma_table_info('provider_connections')")?;
            Ok(stmt
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?)
        })
        .await
        .unwrap();
    assert!(
        columns
            .iter()
            .all(|c| !c.contains("key") || c == "secret_name"),
        "{columns:?}"
    );

    env.store.set_active_connection(id, 2).await.unwrap();
    env.store
        .set_connection_model(id, "gemini-a".into(), 3)
        .await
        .unwrap();
    let active = env.store.active_connection().await.unwrap().unwrap();
    assert_eq!((active.id, active.model.as_deref()), (id, Some("gemini-a")));

    let secret_name = env.store.delete_connection(id, 4).await.unwrap();
    assert!(secret_name.starts_with("gemini-"));
    assert!(
        env.store.active_connection().await.unwrap().is_none(),
        "removing it clears the selection"
    );
}

#[tokio::test]
async fn persona_is_versioned_and_old_versions_remain() {
    let env = env();
    let v1 = env.store.persona(1).await.unwrap();
    assert_eq!(v1.version, 1);
    let v2 = env
        .store
        .save_persona("Astra".into(), "jucăuș".into(), "scurt".into(), 2)
        .await
        .unwrap();
    assert_eq!(v2.version, 2);
    assert_eq!(env.store.persona(3).await.unwrap(), v2);
    assert_eq!(
        env.store.persona_version(1).await.unwrap(),
        v1,
        "a message made with v1 can still be explained"
    );
}

#[tokio::test]
async fn exchanges_are_idempotent_ordered_and_settle_once() {
    let env = env();
    let conn = connection(&env).await;

    let first = env
        .store
        .append_exchange(exchange(None, "k1", "Salut", conn), 10)
        .await
        .unwrap();
    assert!(first.created);
    assert_eq!((first.user_message.seq, first.model_message.seq), (1, 2));
    assert_eq!(first.user_message.role, MessageRole::User);
    assert_eq!(first.model_message.status, MessageStatus::Streaming);

    let again = env
        .store
        .append_exchange(exchange(None, "k1", "Salut", conn), 11)
        .await
        .unwrap();
    assert!(!again.created);
    assert_eq!(again.model_message.id, first.model_message.id);
    assert!(matches!(
        env.store
            .append_exchange(exchange(None, "k1", "Altceva", conn), 12)
            .await,
        Err(StorageError::IdempotencyConflict(_))
    ));

    let second = env
        .store
        .append_exchange(
            exchange(Some(first.conversation_id), "k2", "Ce faci?", conn),
            13,
        )
        .await
        .unwrap();
    assert_eq!((second.user_message.seq, second.model_message.seq), (3, 4));

    let settled = env
        .store
        .settle_message(
            first.model_message.id,
            MessageStatus::Complete,
            Settlement {
                text: "Bună!".into(),
                prompt_tokens: Some(5),
                output_tokens: Some(2),
                ..Default::default()
            },
            14,
        )
        .await
        .unwrap();
    assert_eq!(
        (settled.status, settled.text.as_str()),
        (MessageStatus::Complete, "Bună!")
    );
    let twice = env
        .store
        .settle_message(
            first.model_message.id,
            MessageStatus::Cancelled,
            Settlement::default(),
            15,
        )
        .await;
    assert!(twice.is_err(), "a settled message must not change again");

    let (conversation, messages) = env.store.messages(first.conversation_id, 3).await.unwrap();
    assert_eq!(conversation.title, "Salut");
    assert_eq!(
        messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
        [2, 3, 4],
        "the last N, oldest first"
    );
}

#[tokio::test]
async fn streaming_replies_left_by_a_restart_are_marked_interrupted() {
    let env = env();
    let conn = connection(&env).await;
    let ex = env
        .store
        .append_exchange(exchange(None, "k", "Salut", conn), 1)
        .await
        .unwrap();

    assert_eq!(env.store.fail_interrupted_messages(2).await.unwrap(), 1);
    let msg = env.store.message(ex.model_message.id).await.unwrap();
    assert_eq!(msg.status, MessageStatus::Failed);
    assert_eq!(msg.error_code.as_deref(), Some("Interrupted"));
}
