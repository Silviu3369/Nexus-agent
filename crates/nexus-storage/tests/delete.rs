//! Deleting tasks and exchanges on a real SQLite file: what goes, what
//! stays, what is refused, and that the deleted text is gone from the
//! database file and its journal, not just unlinked.

use std::path::{Path, PathBuf};

use nexus_core::{AgentStepKind, MemoryScope, MemorySource, MessageStatus};
use nexus_protocol::{ConnectionId, ConversationId, MessageId};
use nexus_storage::{Exchange, NewExchange, NewMemory, Settlement, StorageError, Store};
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
    let dir = std::env::temp_dir().join(format!("nexus-delete-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

async fn connection(env: &Env) -> ConnectionId {
    env.store.persona(0).await.unwrap();
    env.store.ensure_default_project(0).await.unwrap();
    env.store
        .save_connection(
            None,
            "Gemini",
            "Gemini".into(),
            false,
            format!("gemini-{}", uuid::Uuid::new_v4()),
            1,
        )
        .await
        .unwrap()
        .id
}

/// A request and its settled reply.
async fn turn(
    env: &Env,
    connection: ConnectionId,
    conversation: Option<ConversationId>,
    request: &str,
    reply: &str,
) -> Exchange {
    let sent = env
        .store
        .append_exchange(
            NewExchange {
                conversation_id: conversation,
                project_id: None,
                idempotency_key: uuid::Uuid::new_v4().to_string(),
                text: request.to_string(),
                connection_id: connection,
                model: "gemini-test".to_string(),
                persona_version: 1,
            },
            1,
        )
        .await
        .unwrap();
    let step = env
        .store
        .begin_step(
            sent.model_message.id,
            AgentStepKind::ModelCall,
            json!({"model": "gemini-test"}),
            2,
        )
        .await
        .unwrap();
    env.store
        .finish_step(step.id, nexus_core::AgentStepStatus::Done, json!({}), 3)
        .await
        .unwrap();
    env.store
        .settle_message(
            sent.model_message.id,
            MessageStatus::Complete,
            Settlement {
                text: reply.to_string(),
                ..Default::default()
            },
            4,
        )
        .await
        .unwrap();
    sent
}

fn texts(messages: &[nexus_storage::MessageRow]) -> Vec<&str> {
    messages.iter().map(|m| m.text.as_str()).collect()
}

/// The database file and its journal, as bytes on disk.
fn on_disk(dir: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    for name in ["nexus.db", "nexus.db-wal"] {
        if let Ok(b) = std::fs::read(dir.join(name)) {
            bytes.extend(b);
        }
    }
    bytes
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[tokio::test]
async fn a_deleted_task_takes_its_messages_and_steps_but_not_the_facts_it_proposed() {
    let env = env();
    let connection = connection(&env).await;
    let first = turn(&env, connection, None, "Plan my week", "Here is a plan").await;
    let task = first.conversation_id;
    turn(&env, connection, Some(task), "Shorter", "Short plan").await;
    let fact = env
        .store
        .add_memory(
            NewMemory {
                scope: MemoryScope::Personal,
                project_id: None,
                content: "The user plans weekly.".into(),
                source: MemorySource::Agent,
                conversation_id: Some(task),
                message_id: Some(first.model_message.id),
                influences: Vec::new(),
            },
            5,
        )
        .await
        .unwrap();
    let other = turn(&env, connection, None, "Another task", "ok").await;

    let deleted = env.store.delete_conversation(task, 6).await.unwrap();
    assert_eq!(deleted.messages, 4);
    assert!(deleted.conversation_deleted);
    assert!(matches!(
        env.store.messages(task, 50).await,
        Err(StorageError::NotFound(_))
    ));
    assert!(
        env.store
            .steps_for_conversation(task)
            .await
            .unwrap()
            .is_empty()
    );
    let kept = env.store.memory(fact.id).await.unwrap();
    assert_eq!(kept.content, "The user plans weekly.", "the fact stays");
    assert_eq!(
        (kept.conversation_id, kept.conversation_title.as_deref()),
        (None, None),
        "it only loses the link to the deleted task"
    );
    let remaining = env.store.conversations(10).await.unwrap();
    assert_eq!(
        remaining.iter().map(|c| c.id).collect::<Vec<_>>(),
        vec![other.conversation_id]
    );
    let events = env.store.events_since(0, 500).await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == "ConversationDeleted" && e.entity_id == task.to_string())
    );
    assert!(matches!(
        env.store.delete_conversation(task, 7).await,
        Err(StorageError::NotFound(_))
    ));
}

#[tokio::test]
async fn a_reply_still_being_written_blocks_the_deletion() {
    let env = env();
    let connection = connection(&env).await;
    let writing = env
        .store
        .append_exchange(
            NewExchange {
                conversation_id: None,
                project_id: None,
                idempotency_key: "w1".into(),
                text: "Write a long essay".into(),
                connection_id: connection,
                model: "gemini-test".into(),
                persona_version: 1,
            },
            1,
        )
        .await
        .unwrap();
    for refused in [
        env.store
            .delete_conversation(writing.conversation_id, 2)
            .await,
        env.store.delete_exchange(writing.user_message.id, 2).await,
        env.store.delete_exchange(writing.model_message.id, 2).await,
    ] {
        assert!(matches!(refused, Err(StorageError::ReplyInProgress)));
    }
    let (_, messages) = env
        .store
        .messages(writing.conversation_id, 10)
        .await
        .unwrap();
    assert_eq!(messages.len(), 2, "nothing was removed");
}

#[tokio::test]
async fn deleting_an_exchange_removes_the_pair_and_retitles_the_task() {
    let env = env();
    let connection = connection(&env).await;
    let first = turn(&env, connection, None, "My secret plan", "Noted").await;
    let task = first.conversation_id;
    let second = turn(&env, connection, Some(task), "What now?", "Rest").await;
    turn(&env, connection, Some(task), "And then?", "Work").await;

    // Named by its reply: the request it answers goes with it.
    let deleted = env
        .store
        .delete_exchange(second.model_message.id, 5)
        .await
        .unwrap();
    assert_eq!((deleted.messages, deleted.conversation_deleted), (2, false));
    let (conversation, messages) = env.store.messages(task, 50).await.unwrap();
    assert_eq!(
        texts(&messages),
        ["My secret plan", "Noted", "And then?", "Work"]
    );
    assert_eq!(conversation.title, "My secret plan");

    // Named by its request; the title must not keep the deleted text.
    env.store
        .delete_exchange(first.user_message.id, 6)
        .await
        .unwrap();
    let (conversation, messages) = env.store.messages(task, 50).await.unwrap();
    assert_eq!(texts(&messages), ["And then?", "Work"]);
    assert_eq!(conversation.title, "And then?");

    // A new turn continues after the remaining ones.
    let next = turn(&env, connection, Some(task), "Finally?", "Sleep").await;
    assert!(next.user_message.seq > messages[1].seq);

    let last = env.store.messages(task, 50).await.unwrap().1;
    for m in last
        .iter()
        .filter(|m| m.role == nexus_core::MessageRole::User)
    {
        env.store.delete_exchange(m.id, 7).await.unwrap();
    }
    assert!(
        matches!(
            env.store.messages(task, 50).await,
            Err(StorageError::NotFound(_))
        ),
        "deleting the last exchange deletes the task"
    );
    assert!(matches!(
        env.store.delete_exchange(MessageId::new(), 8).await,
        Err(StorageError::NotFound(_))
    ));
}

#[tokio::test]
async fn deleted_text_is_gone_from_the_database_file_and_its_journal() {
    let env = env();
    let connection = connection(&env).await;
    let request = format!("SENTINEL-request-{}", uuid::Uuid::new_v4());
    let reply = format!("SENTINEL-reply-{}", uuid::Uuid::new_v4());
    let kept = format!("SENTINEL-kept-{}", uuid::Uuid::new_v4());
    let gone = turn(&env, connection, None, &request, &reply).await;
    let stays = turn(&env, connection, None, &kept, "ok").await;
    let before = on_disk(&env.dir);
    assert!(
        contains(&before, &request) && contains(&before, &reply),
        "the check can see stored text"
    );

    env.store
        .delete_conversation(gone.conversation_id, 5)
        .await
        .unwrap();
    let after = on_disk(&env.dir);
    assert!(
        !contains(&after, &request),
        "the deleted request is not on disk"
    );
    assert!(
        !contains(&after, &reply),
        "the deleted reply is not on disk"
    );
    assert!(contains(&after, &kept), "other tasks are untouched");
    assert_eq!(env.store.integrity_check().await.unwrap(), "ok");
    assert!(env.store.messages(stays.conversation_id, 10).await.is_ok());
}
