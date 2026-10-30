//! Deleting tasks and exchanges through the real host. A local Gemini
//! stand-in records what would have left the PC, so the check is on what
//! the model is sent after a deletion, not only on what the UI lists.

mod support;

use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_protocol::{AddMemoryRequest, ChatStatus, ErrorCode, MemoryQuery, MemoryScope};
use support::TestHost;
use support::chat::{gemini, message, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply, text_event};

const TOKEN: &str = "delete-token";
const KEY: &str = "SENTINEL-gemini-key-delete-77b2";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(10),
        retry_waits: Vec::new(),
        ..RuntimeConfig::default()
    }
}

async fn connect_gemini(client: &NexusClient) {
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_deleted_exchange_is_no_longer_sent_to_the_model() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(Reply::text(&["Noted your code."]));
    let first = send(&client, None, "d1", "My door code is 4711").await;
    settled(&client, &first).await;
    let task = first.conversation_id;
    fake.push(Reply::text(&["Sure."]));
    let second = send(&client, Some(task), "d2", "Plan dinner").await;
    settled(&client, &second).await;

    let deleted = client.delete_exchange(first.user_message_id).await.unwrap();
    assert_eq!((deleted.messages, deleted.conversation_deleted), (2, false));
    let view = client.conversation(task).await.unwrap();
    assert_eq!(view.messages.len(), 2);
    assert_eq!(
        view.title, "Plan dinner",
        "the title no longer holds deleted text"
    );
    let summaries = client.conversations(10).await.unwrap();
    assert!(summaries.iter().all(|c| !c.title.contains("4711")));

    fake.push(Reply::text(&["Pasta."]));
    let third = send(&client, Some(task), "d3", "What did I say?").await;
    settled(&client, &third).await;
    let body = fake.generations().pop().unwrap().json();
    let sent = body["contents"].to_string();
    assert!(
        !sent.contains("4711"),
        "the deleted request is not sent: {sent}"
    );
    assert!(!sent.contains("Noted your code."), "nor its reply");
    assert!(
        sent.contains("Plan dinner"),
        "the rest of the task still is"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_task_is_deleted_whole_but_not_while_its_reply_is_being_written() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(Reply::SseThenHang(vec![text_event("Starting ")]));
    let sent = send(&client, None, "w1", "Write a long essay").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while message(&client, sent.conversation_id, sent.reply_message_id)
        .await
        .text
        .is_empty()
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the reply never started"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    for refused in [
        client.delete_conversation(sent.conversation_id).await,
        client.delete_exchange(sent.reply_message_id).await,
    ] {
        assert_eq!(
            refused.unwrap_err().code(),
            Some(ErrorCode::InvalidRequest),
            "a reply being written is stopped first"
        );
    }
    assert_eq!(
        message(&client, sent.conversation_id, sent.reply_message_id)
            .await
            .status,
        ChatStatus::Streaming,
        "the refused deletion left the reply running"
    );

    client.cancel_reply(sent.reply_message_id).await.unwrap();
    let fact = client
        .add_memory(AddMemoryRequest {
            scope: MemoryScope::Personal,
            project_id: None,
            content: "Likes essays.".into(),
        })
        .await
        .unwrap();
    let deleted = client
        .delete_conversation(sent.conversation_id)
        .await
        .unwrap();
    assert!(deleted.conversation_deleted);
    assert_eq!(deleted.messages, 2);
    assert_eq!(
        client
            .conversation(sent.conversation_id)
            .await
            .unwrap_err()
            .code(),
        Some(ErrorCode::NotFound)
    );
    assert!(client.conversations(10).await.unwrap().is_empty());
    assert!(
        client
            .activity(50)
            .await
            .unwrap()
            .iter()
            .all(|s| s.conversation_id != sent.conversation_id),
        "its steps are gone from the activity"
    );
    let facts = client
        .memory(MemoryQuery {
            limit: 10,
            ..MemoryQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(
        facts.iter().map(|m| m.memory_id).collect::<Vec<_>>(),
        vec![fact.memory_id],
        "memory is managed on its own"
    );
    let followup = client
        .send_message(nexus_protocol::SendMessageRequest {
            conversation_id: Some(sent.conversation_id),
            project_id: None,
            idempotency_key: "w2".into(),
            text: "More".into(),
            agent_id: None,
        })
        .await
        .unwrap_err();
    assert_eq!(followup.code(), Some(ErrorCode::NotFound));
    host.shutdown().await;
}
