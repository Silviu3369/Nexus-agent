//! Providers through the real host: a temporary outage is tried again, the
//! user's backup model takes over only when chosen, and a key's provider is
//! read from its format — never by trying it against several providers.
//! No real provider is called; local stand-ins record what was sent.

mod support;

use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_protocol::{
    AgentStepDetail, AgentStepStatus, ChatStatus, ErrorCode, ProviderChoice, SaveConnectionRequest,
};
use support::TestHost;
use support::chat::{gemini, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};
use support::fake_provider::{Canned, FakeProvider};

const TOKEN: &str = "providers-token";
const KEY: &str = "SENTINEL-gemini-key-providers-7d21";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(10),
        retry_waits: vec![Duration::from_millis(50), Duration::from_millis(50)],
        ..RuntimeConfig::default()
    }
}

async fn connect_gemini(client: &NexusClient) -> nexus_protocol::ConnectionId {
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
    saved.connection_id
}

fn overloaded() -> Reply {
    Reply::status(
        503,
        r#"{"error":{"code":503,"message":"This model is currently experiencing high demand.","status":"UNAVAILABLE"}}"#,
    )
}

fn model_calls(
    reply: &nexus_protocol::ChatMessageView,
) -> Vec<(AgentStepStatus, Option<u32>, Option<bool>, String)> {
    reply
        .steps
        .iter()
        .filter_map(|s| match &s.detail {
            AgentStepDetail::ModelCall {
                model,
                attempt,
                backup,
                ..
            } => Some((s.status, *attempt, *backup, model.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_temporary_outage_is_tried_again_and_each_try_is_a_visible_step() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(overloaded());
    fake.push(overloaded());
    fake.push(Reply::text(&["Gata."]));
    let sent = send(&client, None, "r1", "Salut").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Complete);
    assert_eq!(reply.text, "Gata.");
    assert_eq!(
        model_calls(&reply)
            .iter()
            .map(|(s, a, b, _)| (*s, *a, *b))
            .collect::<Vec<_>>(),
        vec![
            (AgentStepStatus::Failed, None, None),
            (AgentStepStatus::Failed, Some(2), None),
            (AgentStepStatus::Done, Some(3), None),
        ]
    );
    assert_eq!(fake.generations().len(), 3);

    // Three outages in a row and no backup chosen: the reply fails, clearly.
    for _ in 0..3 {
        fake.push(overloaded());
    }
    let sent = send(&client, Some(sent.conversation_id), "r2", "Încă o dată").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Failed);
    assert_eq!(reply.failure.as_ref().unwrap().code, "ProviderUnavailable");
    assert_eq!(model_calls(&reply).len(), 3, "at most three tries");
    host.shutdown().await;
}

#[tokio::test]
async fn a_refused_key_or_a_used_up_quota_is_never_tried_again() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect_gemini(&client).await;

    fake.push(Reply::status(429, "{}"));
    let sent = send(&client, None, "q1", "Salut").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.failure.as_ref().unwrap().code, "QuotaExceeded");
    assert_eq!(model_calls(&reply).len(), 1);

    fake.push(Reply::status(400, r#"{"error":{"message":"bad request"}}"#));
    let sent = send(&client, Some(sent.conversation_id), "q2", "Salut").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.failure.as_ref().unwrap().code, "InvalidRequest");
    assert_eq!(model_calls(&reply).len(), 1);
    assert_eq!(fake.generations().len(), 2);
    host.shutdown().await;
}

#[tokio::test]
async fn the_backup_model_takes_over_only_when_chosen_and_says_so() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    let connection = connect_gemini(&client).await;

    assert!(client.backup_model().await.unwrap().is_none());
    let chosen = client
        .set_backup_model(Some(connection), Some("gemini-fake-backup".into()))
        .await
        .unwrap()
        .expect("the backup is set");
    assert_eq!(chosen.model, "gemini-fake-backup");
    assert_eq!(chosen.provider, "Gemini");

    for _ in 0..3 {
        fake.push(overloaded());
    }
    fake.push(Reply::text(&["Din rezervă."]));
    let sent = send(&client, None, "b1", "Salut").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Complete);
    assert_eq!(reply.text, "Din rezervă.");
    let calls = model_calls(&reply);
    assert_eq!(calls.len(), 4);
    assert_eq!(
        calls[3],
        (
            AgentStepStatus::Done,
            None,
            Some(true),
            "gemini-fake-backup".into()
        )
    );
    let last = fake.generations().pop().unwrap();
    assert!(
        last.target.contains("gemini-fake-backup"),
        "{}",
        last.target
    );

    // Clearing it restores the rule: no fallback.
    assert!(client.set_backup_model(None, None).await.unwrap().is_none());
    let half = client
        .set_backup_model(Some(connection), None)
        .await
        .unwrap_err();
    assert_eq!(half.code(), Some(ErrorCode::InvalidRequest));
    host.shutdown().await;
}

#[tokio::test]
async fn a_key_whose_provider_cannot_be_told_is_refused_without_any_call() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    for (key, says) in [
        ("sk-could-be-either", "OpenAI or DeepSeek"),
        ("plainly-unknown", "choose the provider"),
    ] {
        let err = client
            .save_connection(SaveConnectionRequest {
                connection_id: None,
                display_name: None,
                provider: None,
                free_tier: false,
                api_key: Some(key.to_string()),
            })
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::InvalidRequest));
        assert!(err.to_string().contains(says), "{err}");
    }
    assert!(fake.received().is_empty(), "no provider was contacted");
    assert!(client.list_connections().await.unwrap().is_empty());
    host.shutdown().await;
}

#[tokio::test]
async fn an_anthropic_key_is_detected_checked_and_used_end_to_end() {
    let gemini_fake = FakeGemini::start(KEY).await;
    let claude = FakeProvider::start(serde_json::json!({
        "data": [{"type": "model", "id": "claude-fake", "display_name": "Claude Fake"}],
        "has_more": false
    }))
    .await;
    let host = TestHost::start_with(
        TOKEN,
        RuntimeConfig {
            anthropic_base_url: claude.base_url.clone(),
            ..config(&gemini_fake)
        },
    )
    .await;
    let client = host.client().await;
    let key = "sk-ant-api03-SENTINEL-providers-claude";

    let saved = client
        .save_connection(SaveConnectionRequest {
            connection_id: None,
            display_name: None,
            provider: None,
            // Ignored: only Google AI Studio has a free tier.
            free_tier: true,
            api_key: Some(key.to_string()),
        })
        .await
        .unwrap();
    assert_eq!(saved.provider, "Anthropic");
    assert_eq!(saved.display_name, "Anthropic (Claude)");
    assert!(!saved.free_tier);
    assert!(saved.has_key);
    let models = client.list_models(saved.connection_id).await.unwrap();
    assert_eq!(models[0].id, "claude-fake");
    client
        .select_model(saved.connection_id, "claude-fake".into())
        .await
        .unwrap();

    claude.push(Canned::Sse(vec![
        r#"{"type":"message_start","message":{"usage":{"input_tokens":8,"output_tokens":1}}}"#.into(),
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Salut de la Claude."}}"#.into(),
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#.into(),
        r#"{"type":"message_stop"}"#.into(),
    ]));
    let sent = send(&client, None, "c1", "Salut").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Complete);
    assert_eq!(reply.text, "Salut de la Claude.");
    assert_eq!(reply.model.as_deref(), Some("claude-fake"));
    assert_eq!(model_calls(&reply)[0].3, "claude-fake");

    let received = claude.received();
    let post = received.iter().find(|r| r.method == "POST").unwrap();
    assert_eq!(post.target, "/v1/messages");
    assert_eq!(post.header("x-api-key"), Some(key));
    assert!(
        received.iter().all(|r| !r.target.contains(key)),
        "the key never goes in a URL"
    );
    assert!(
        gemini_fake.received().is_empty(),
        "Google was never contacted"
    );

    // A connection keeps its provider: the explicit choice is ignored on update.
    let updated = client
        .save_connection(SaveConnectionRequest {
            connection_id: Some(saved.connection_id),
            display_name: Some("Claude".into()),
            provider: Some(ProviderChoice::OpenAi),
            free_tier: false,
            api_key: None,
        })
        .await
        .unwrap();
    assert_eq!(updated.provider, "Anthropic");
    assert_eq!(updated.display_name, "Claude");
    host.shutdown().await;
}
