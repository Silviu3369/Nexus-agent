//! M2 persistence on a real SQLite file: schema migration, connections
//! without secrets, versioned persona and conversations.

mod common;

use std::path::PathBuf;

use nexus_core::{AgentStepKind, AgentStepStatus, MessageRole, MessageStatus, ThinkingLevel};
use nexus_protocol::ConnectionId;
use nexus_storage::{NewExchange, SUPPORTED_VERSION, Settlement, StorageError, Store};
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
    let dir = std::env::temp_dir().join(format!("nexus-chat-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

/// A connection plus persona v1 and the default project (messages and
/// conversations reference them by foreign key).
async fn connection(env: &Env) -> ConnectionId {
    env.store.persona(0).await.unwrap();
    env.store.ensure_default_project(0).await.unwrap();
    env.store
        .save_connection(
            None,
            "Gemini",
            "Gemini personal".into(),
            true,
            format!("gemini-{}", uuid::Uuid::new_v4()),
            1,
        )
        .await
        .unwrap()
        .id
}

/// Puts a file back as it was before schema v6 (no paused turns from v8, no
/// MCP tables from v7, no memory, no project on conversations, no project
/// description), keeping every row.
const UNDO_V6: &str = "PRAGMA foreign_keys = OFF;
    DROP TABLE inbox_files; DROP TABLE register_inboxes; DROP TABLE folders; DROP TABLE record_changes; DROP TABLE records; DROP TABLE registers; DROP TABLE automation_runs; DROP TABLE automations; DROP TABLE model_usage; DROP TABLE model_prices; DROP TABLE paused_turns; ALTER TABLE runs DROP COLUMN outcome_json;
    DROP TABLE mcp_tools; DROP TABLE mcp_servers;
    DROP TRIGGER memory_items_ai; DROP TRIGGER memory_items_ad; DROP TRIGGER memory_items_au;
    DROP TABLE memory_fts; DROP TABLE memory_items;
    DROP INDEX conversations_by_project;
    CREATE TABLE conversations_v5 (id TEXT PRIMARY KEY, title TEXT NOT NULL,
        created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL) STRICT;
    INSERT INTO conversations_v5 SELECT id, title, created_at_ms, updated_at_ms FROM conversations;
    DROP TABLE conversations;
    ALTER TABLE conversations_v5 RENAME TO conversations;
    ALTER TABLE projects DROP COLUMN description;
    PRAGMA foreign_keys = ON;";

fn exchange(
    conversation: Option<nexus_protocol::ConversationId>,
    key: &str,
    text: &str,
    connection: ConnectionId,
) -> NewExchange {
    NewExchange {
        requester: nexus_storage::Requester::User,
        pack: None,
        agent: None,
        conversation_id: conversation,
        project_id: None,
        idempotency_key: key.to_string(),
        text: text.to_string(),
        connection_id: connection,
        model: "gemini-test".to_string(),
        persona_version: 1,
        title: None,
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
            .submit_task(
                nexus_storage::EffectRequester::User,
                project,
                "k".into(),
                "{}".into(),
                "h".into(),
                2,
            )
            .await
            .unwrap();
        // Roll the file back to version 1 as an M1 host would have left it.
        store
            .call(|c| {
                common::undo_v15(c)?;
                c.execute_batch(UNDO_V6)?;
                c.execute_batch(
                    "DROP TABLE agent_steps; DROP TABLE messages; DROP TABLE conversations;
                     DROP TABLE personas; DROP TABLE provider_connections; DROP TABLE settings;
                     PRAGMA user_version = 1;",
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
                "Gemini",
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
                common::undo_v15(c)?;
                c.execute_batch(UNDO_V6)?;
                c.execute_batch(
                    "DROP TABLE agent_steps; ALTER TABLE provider_connections DROP COLUMN thinking_level;
                     PRAGMA user_version = 2;",
                )?;
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
async fn migrating_to_v5_keeps_every_connection_and_the_messages_linked_to_it() {
    let dir = std::env::temp_dir().join(format!("nexus-migrate-v5-test-{}", uuid::Uuid::new_v4()));
    let path = dir.join("nexus.db");
    let (id, reply) = {
        let store = Store::open(&path).unwrap();
        store.persona(0).await.unwrap();
        store.ensure_default_project(0).await.unwrap();
        let row = store
            .save_connection(
                None,
                "Gemini",
                "Mine".into(),
                true,
                "gemini-v4-test".into(),
                1,
            )
            .await
            .unwrap();
        store
            .set_thinking_level(row.id, ThinkingLevel::High, 2)
            .await
            .unwrap();
        store
            .set_connection_model(row.id, "gemini-3.8-flash".into(), 3)
            .await
            .unwrap();
        let sent = store
            .append_exchange(exchange(None, "k-v5", "salut", row.id), 4)
            .await
            .unwrap();
        // Put back the v4 table (Gemini only), as a host before this change left it.
        store
            .call(|c| {
                common::undo_v15(c)?;
                c.execute_batch(UNDO_V6)?;
                c.execute_batch(
                    "PRAGMA foreign_keys = OFF;
                     CREATE TABLE old (
                         id TEXT PRIMARY KEY,
                         provider TEXT NOT NULL CHECK (provider IN ('Gemini')),
                         display_name TEXT NOT NULL, model TEXT, free_tier INTEGER NOT NULL,
                         secret_name TEXT NOT NULL UNIQUE, revision INTEGER NOT NULL,
                         created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
                         thinking_level TEXT NOT NULL DEFAULT 'Low') STRICT;
                     INSERT INTO old SELECT * FROM provider_connections;
                     DROP TABLE provider_connections;
                     ALTER TABLE old RENAME TO provider_connections;
                     PRAGMA foreign_keys = ON;
                     PRAGMA user_version = 4;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        (row.id, sent.model_message.id)
    };

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let row = store.connection(id).await.unwrap();
    assert_eq!(row.provider, "Gemini");
    assert_eq!(row.display_name, "Mine");
    assert_eq!(row.model.as_deref(), Some("gemini-3.8-flash"));
    assert_eq!(row.thinking_level, ThinkingLevel::High);
    assert_eq!(row.secret_name, "gemini-v4-test");
    assert!(row.free_tier);
    let linked: Option<String> = store
        .call(move |c| {
            Ok(c.query_row(
                "SELECT connection_id FROM messages WHERE id = ?1",
                [reply.to_string()],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(
        linked,
        Some(id.to_string()),
        "the rebuild must not clear message links"
    );
    let enforced: i64 = store
        .call(|c| Ok(c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(
        enforced, 1,
        "foreign keys are enforced again after the rebuild"
    );

    for provider in ["OpenAI", "Anthropic", "DeepSeek"] {
        store
            .save_connection(
                None,
                provider,
                provider.into(),
                false,
                format!("{provider}-v5"),
                5,
            )
            .await
            .unwrap();
    }
    assert!(
        store
            .save_connection(None, "Other", "x".into(), false, "other-v5".into(), 6)
            .await
            .is_err(),
        "only known providers are accepted"
    );
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn migrating_to_v6_puts_every_conversation_in_the_default_project_and_keeps_its_steps() {
    let dir = std::env::temp_dir().join(format!("nexus-migrate-v6-test-{}", uuid::Uuid::new_v4()));
    let path = dir.join("nexus.db");
    let (project, conversation) = {
        let env_store = Store::open(&path).unwrap();
        env_store.persona(0).await.unwrap();
        let project = env_store.ensure_default_project(0).await.unwrap();
        let connection = env_store
            .save_connection(
                None,
                "Gemini",
                "g".into(),
                false,
                "gemini-v6-test".into(),
                1,
            )
            .await
            .unwrap()
            .id;
        let sent = env_store
            .append_exchange(exchange(None, "k-v6", "Salut", connection), 2)
            .await
            .unwrap();
        let step = env_store
            .begin_step(
                sent.model_message.id,
                AgentStepKind::ModelCall,
                json!({"model": "m"}),
                3,
            )
            .await
            .unwrap();
        env_store
            .finish_step(step.id, AgentStepStatus::Done, json!({"duration_ms": 5}), 4)
            .await
            .unwrap();
        env_store
            .call(|c| {
                common::undo_v15(c)?;
                c.execute_batch(UNDO_V6)?;
                c.execute_batch("PRAGMA user_version = 5;")?;
                Ok(())
            })
            .await
            .unwrap();
        (project, sent.conversation_id)
    };

    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().await.unwrap(), SUPPORTED_VERSION);
    assert_eq!(
        store.conversation_project(conversation).await.unwrap(),
        project
    );
    let steps = store.steps_for_conversation(conversation).await.unwrap();
    assert_eq!(steps.len(), 1, "the steps table was rebuilt with its rows");
    assert_eq!(steps[0].detail, json!({"model": "m", "duration_ms": 5}));
    let enforced: i64 = store
        .call(|c| Ok(c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(enforced, 1);
    assert_eq!(store.projects().await.unwrap()[0].conversations, 1);
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn the_backup_model_is_explicit_and_goes_away_with_its_connection() {
    let env = env();
    let id = connection(&env).await;
    assert!(env.store.backup_model().await.unwrap().is_none());
    env.store
        .set_backup_model(Some((id, "gemini-backup".into())), 2)
        .await
        .unwrap();
    let (row, model) = env.store.backup_model().await.unwrap().unwrap();
    assert_eq!((row.id, model.as_str()), (id, "gemini-backup"));
    env.store.set_backup_model(None, 3).await.unwrap();
    assert!(env.store.backup_model().await.unwrap().is_none());

    env.store
        .set_backup_model(Some((id, "gemini-backup".into())), 4)
        .await
        .unwrap();
    env.store.delete_connection(id, 5).await.unwrap();
    assert!(env.store.backup_model().await.unwrap().is_none());
    assert!(
        env.store
            .set_backup_model(Some((id, "x".into())), 6)
            .await
            .is_err(),
        "a backup must name an existing connection"
    );
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

#[tokio::test]
async fn new_closed_columns_accept_exactly_the_domain_values() {
    // Keeps the SQL CHECK lists and the nexus-core enums from drifting apart.
    let env = env();
    let conn = connection(&env).await;
    let message = env
        .store
        .append_exchange(exchange(None, "k", "Salut", conn), 1)
        .await
        .unwrap()
        .model_message
        .id
        .to_string();
    let rejected = env
        .store
        .call(move |c| {
            let mut rejected = Vec::new();
            let mut seq = 0;
            for kind in AgentStepKind::ALL {
                for status in AgentStepStatus::ALL {
                    seq += 1;
                    let finished = (*status != AgentStepStatus::Running).then_some(1i64);
                    c.execute(
                        "INSERT INTO agent_steps VALUES (?1, ?2, ?3, ?4, ?5, '{}', 0, ?6)",
                        (format!("s{seq}"), &message, seq, kind.as_str(), status.as_str(), finished),
                    )?;
                }
            }
            if c.execute(
                "INSERT INTO agent_steps VALUES ('sx', ?1, 99, 'ShellCommand', 'Running', '{}', 0, NULL)",
                [&message],
            )
            .is_err()
            {
                rejected.push("kind");
            }
            if c.execute(
                "INSERT INTO agent_steps VALUES ('sy', ?1, 98, 'Fetch', 'Approved', '{}', 0, 1)",
                [&message],
            )
            .is_err()
            {
                rejected.push("status");
            }
            for level in ThinkingLevel::ALL {
                c.execute(
                    "UPDATE provider_connections SET thinking_level = ?1",
                    [level.as_str()],
                )?;
            }
            if c.execute("UPDATE provider_connections SET thinking_level = 'Max'", []).is_err() {
                rejected.push("thinking_level");
            }
            Ok(rejected)
        })
        .await
        .unwrap();
    assert_eq!(rejected, ["kind", "status", "thinking_level"]);
}

#[tokio::test]
async fn agent_steps_are_ordered_settled_once_and_keep_their_start_detail() {
    let env = env();
    let conn = connection(&env).await;
    let ex = env
        .store
        .append_exchange(exchange(None, "k", "Caută ceva", conn), 1)
        .await
        .unwrap();
    let reply = ex.model_message.id;

    let call = env
        .store
        .begin_step(
            reply,
            AgentStepKind::ModelCall,
            json!({"model": "gemini-x"}),
            10,
        )
        .await
        .unwrap();
    assert_eq!((call.seq, call.status), (1, AgentStepStatus::Running));
    assert_eq!(call.finished_at_ms, None);
    assert_eq!(call.conversation_id, ex.conversation_id);
    assert_eq!(call.conversation_title, "Caută ceva");

    let fetch = env
        .store
        .begin_step(
            reply,
            AgentStepKind::Fetch,
            json!({"url": "https://example.com"}),
            11,
        )
        .await
        .unwrap();
    assert_eq!(fetch.seq, 2);

    let done = env
        .store
        .finish_step(
            fetch.id,
            AgentStepStatus::Done,
            json!({"http_status": 200, "bytes": 171}),
            12,
        )
        .await
        .unwrap();
    assert_eq!(done.status, AgentStepStatus::Done);
    assert_eq!(done.finished_at_ms, Some(12));
    assert_eq!(
        done.detail,
        json!({"url": "https://example.com", "http_status": 200, "bytes": 171}),
        "the finish detail is merged into the start detail"
    );
    assert!(
        env.store
            .finish_step(fetch.id, AgentStepStatus::Failed, json!({}), 13)
            .await
            .is_err(),
        "a settled step must not change again"
    );

    let in_conversation = env
        .store
        .steps_for_conversation(ex.conversation_id)
        .await
        .unwrap();
    assert_eq!(
        in_conversation.iter().map(|s| s.seq).collect::<Vec<_>>(),
        [1, 2]
    );
    let recent = env.store.recent_steps(10).await.unwrap();
    assert_eq!(
        recent.iter().map(|s| s.id).collect::<Vec<_>>(),
        [fetch.id, call.id],
        "newest first"
    );
}

#[tokio::test]
async fn running_steps_fail_when_their_reply_stops_or_the_host_restarts() {
    let env = env();
    let conn = connection(&env).await;
    let a = env
        .store
        .append_exchange(exchange(None, "a", "Unu", conn), 1)
        .await
        .unwrap()
        .model_message
        .id;
    let b = env
        .store
        .append_exchange(exchange(None, "b", "Doi", conn), 2)
        .await
        .unwrap()
        .model_message
        .id;
    for message in [a, a, b] {
        env.store
            .begin_step(message, AgentStepKind::ModelCall, json!({}), 3)
            .await
            .unwrap();
    }

    assert_eq!(
        env.store
            .fail_running_steps(Some(a), "Cancelled", 4)
            .await
            .unwrap(),
        2,
        "only the stopped reply's steps"
    );
    assert_eq!(
        env.store
            .fail_running_steps(None, "Interrupted", 5)
            .await
            .unwrap(),
        1
    );
    let all = env.store.recent_steps(10).await.unwrap();
    assert!(all.iter().all(|s| s.status == AgentStepStatus::Failed));
    assert!(
        all.iter()
            .filter(|s| s.message_id == a)
            .all(|s| s.detail["error"] == "Cancelled")
    );
    assert!(
        all.iter()
            .filter(|s| s.message_id == b)
            .all(|s| s.detail["error"] == "Interrupted")
    );
}
