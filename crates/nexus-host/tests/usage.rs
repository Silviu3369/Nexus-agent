//! Usage and the daily token budget through the real host (`PLAN_NEXUS_RO.md`
//! M2): every model call records the tokens the provider reported, today's
//! total is summed per model, a cost is estimated only from prices the user
//! entered (a free tier costs nothing), and the budget stops new requests
//! and replies between steps. Deleting a task does not reset the day.

mod support;

use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_protocol::{
    AgentStepDetail, ChatStatus, ErrorCode, ModelPrice, ProviderChoice, SaveConnectionRequest,
};
use serde_json::json;
use support::TestHost;
use support::chat::{send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};

const TOKEN: &str = "usage-token";
const KEY: &str = "SENTINEL-gemini-key-usage-6d3f";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(10),
        retry_waits: Vec::new(),
        ..RuntimeConfig::default()
    }
}

/// A connection, on a free tier or not, with the fake's model selected.
async fn connect(client: &NexusClient, free_tier: bool) {
    let saved = client
        .save_connection(SaveConnectionRequest {
            connection_id: None,
            display_name: Some("Gemini".to_string()),
            provider: Some(ProviderChoice::Gemini),
            free_tier,
            api_key: Some(KEY.to_string()),
        })
        .await
        .unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
}

#[tokio::test]
async fn each_model_call_records_its_tokens_and_today_is_estimated_from_your_prices() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client, false).await;

    let before = client.usage().await.unwrap();
    assert_eq!(before.tokens_today, 0);
    assert!(
        before.daily_token_budget.is_none(),
        "no limit unless you set one"
    );
    let model = before
        .models
        .iter()
        .find(|m| m.model == MODEL)
        .expect("the chosen model is listed before it is used");
    assert!(model.price.is_none() && model.estimated_cost_today.is_none());
    assert!(
        before.estimated_cost_today.is_none(),
        "no price, no estimate"
    );

    fake.push(Reply::text(&["Hello."]));
    let sent = send(&client, None, "u1", "Hello").await;
    let reply = settled(&client, &sent).await;
    assert!(reply.steps.iter().any(|s| matches!(
        s.detail,
        AgentStepDetail::ModelCall {
            prompt_tokens: Some(11),
            output_tokens: Some(5),
            ..
        }
    )));

    let usage = client
        .set_model_price(
            "Gemini".to_string(),
            MODEL.to_string(),
            Some(ModelPrice {
                input_per_million: 1.0,
                output_per_million: 4.0,
            }),
        )
        .await
        .unwrap();
    assert_eq!(usage.tokens_today, 16);
    let model = usage.models.iter().find(|m| m.model == MODEL).unwrap();
    assert_eq!(model.provider, "Gemini");
    assert_eq!(
        (
            model.calls_today,
            model.prompt_tokens_today,
            model.output_tokens_today
        ),
        (1, 11, 5)
    );
    assert!(!model.free_tier);
    let expected = (11.0 * 1.0 + 5.0 * 4.0) / 1_000_000.0;
    assert!((model.estimated_cost_today.unwrap() - expected).abs() < 1e-12);
    assert!((usage.estimated_cost_today.unwrap() - expected).abs() < 1e-12);

    let cleared = client
        .set_model_price("Gemini".to_string(), MODEL.to_string(), None)
        .await
        .unwrap();
    assert!(cleared.estimated_cost_today.is_none());
    let invalid = client
        .set_model_price(
            "Gemini".to_string(),
            MODEL.to_string(),
            Some(ModelPrice {
                input_per_million: -1.0,
                output_per_million: 1.0,
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.code(), Some(ErrorCode::InvalidRequest));
    host.shutdown().await;
}

#[tokio::test]
async fn a_free_tier_costs_nothing_whatever_the_price() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client, true).await;
    fake.push(Reply::text(&["Hi."]));
    settled(&client, &send(&client, None, "f1", "Hi").await).await;
    let usage = client
        .set_model_price(
            "Gemini".to_string(),
            MODEL.to_string(),
            Some(ModelPrice {
                input_per_million: 1.0,
                output_per_million: 1.0,
            }),
        )
        .await
        .unwrap();
    let model = usage.models.iter().find(|m| m.model == MODEL).unwrap();
    assert!(model.free_tier);
    assert_eq!(model.estimated_cost_today, Some(0.0));
    host.shutdown().await;
}

#[tokio::test]
async fn the_daily_budget_stops_a_reply_between_steps_and_new_requests() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client, false).await;
    let refused_budget = client.set_daily_token_budget(Some(0)).await.unwrap_err();
    assert_eq!(refused_budget.code(), Some(ErrorCode::InvalidRequest));
    let usage = client.set_daily_token_budget(Some(10)).await.unwrap();
    assert_eq!(usage.daily_token_budget, Some(10));

    // The first call spends 13 tokens; the step after it is not started.
    fake.push(Reply::calls(&[(
        "fetch_url",
        json!({"url": "http://127.0.0.1:1/private"}),
    )]));
    fake.push(Reply::text(&["Never sent."]));
    let sent = send(&client, None, "b1", "Read that page").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Failed);
    assert_eq!(reply.failure.as_ref().unwrap().code, "BudgetExceeded");
    assert_eq!(fake.generations().len(), 1, "no call past the budget");

    // A new request is refused before anything is sent.
    let refused = client
        .send_message(nexus_protocol::SendMessageRequest {
            conversation_id: None,
            project_id: None,
            idempotency_key: "b2".to_string(),
            text: "Again".to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(ErrorCode::BudgetExceeded));
    assert_eq!(fake.generations().len(), 1);

    // Deleting the task does not give the tokens back.
    client
        .delete_conversation(sent.conversation_id)
        .await
        .unwrap();
    assert_eq!(client.usage().await.unwrap().tokens_today, 13);
    let still = client
        .send_message(nexus_protocol::SendMessageRequest {
            conversation_id: None,
            project_id: None,
            idempotency_key: "b3".to_string(),
            text: "Again".to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(still.code(), Some(ErrorCode::BudgetExceeded));

    // Raised, the next request goes through.
    client.set_daily_token_budget(Some(1000)).await.unwrap();
    let ok = send(&client, None, "b4", "Again").await;
    assert_eq!(settled(&client, &ok).await.text, "Never sent.");
    let none = client.set_daily_token_budget(None).await.unwrap();
    assert!(none.daily_token_budget.is_none());
    host.shutdown().await;
}
