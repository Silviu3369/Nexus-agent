//! OpenAI, Anthropic and DeepSeek adapters against a controlled local HTTP
//! server. No real call is made; the server records exactly what each
//! adapter sent, so the headers, paths and bodies are checked as sent.

use std::time::Duration;

use nexus_providers::{
    ApiKey, Client, FinishReason, Message, ProviderError, ProviderEvent, ProviderKind,
    ProviderRequest, Role, ThinkingLevel, ToolDeclaration, Usage,
};
use serde_json::json;

mod common;
use common::{Reply, server};

const SENTINEL: &str = "SENTINEL-other-key-91c2";

fn client(kind: ProviderKind, base: &str) -> Client {
    Client::new(kind, ApiKey::new(SENTINEL.to_string()).unwrap(), base).unwrap()
}

fn request(model: &str) -> ProviderRequest {
    ProviderRequest {
        model: model.to_string(),
        system_instruction: "Reply in the user's language.".to_string(),
        messages: vec![Message::text(Role::User, "Salut")],
        tools: vec![ToolDeclaration {
            name: "fetch_url".to_string(),
            description: "Read a public page".to_string(),
            parameters: json!({"type": "object", "properties": {"url": {"type": "string"}}}),
        }],
        max_output_tokens: 256,
        deadline: Duration::from_secs(5),
        thinking_level: Some(ThinkingLevel::Low),
    }
}

async fn collect(
    client: &Client,
    req: &ProviderRequest,
) -> (Result<(), ProviderError>, Vec<ProviderEvent>) {
    let mut events = Vec::new();
    let result = client.stream(req, |e| events.push(e)).await;
    (result, events)
}

#[tokio::test]
async fn openai_streams_text_and_a_tool_call_with_the_key_only_in_the_header() {
    let (base, server) = server(vec![Reply::Sse(vec![
        r#"{"type":"response.created","response":{"status":"in_progress"}}"#,
        r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"Bun"}"#,
        r#"{"type":"response.output_text.delta","item_id":"msg_1","output_index":0,"content_index":0,"delta":"ă!"}"#,
        r#"{"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_1","call_id":"call_7","name":"fetch_url","arguments":"{\"url\":\"https://example.com\"}","status":"completed"}}"#,
        r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":12,"output_tokens":4,"total_tokens":16}}}"#,
    ])])
    .await;
    let (result, events) =
        collect(&client(ProviderKind::OpenAi, &base), &request("gpt-test")).await;
    result.unwrap();
    assert_eq!(
        events,
        vec![
            ProviderEvent::TextDelta("Bun".into()),
            ProviderEvent::TextDelta("ă!".into()),
            ProviderEvent::ToolProposal {
                id: Some("call_7".into()),
                name: "fetch_url".into(),
                arguments: json!({"url": "https://example.com"}),
            },
            ProviderEvent::Usage(Usage {
                prompt_tokens: 12,
                output_tokens: 4,
                total_tokens: 16
            }),
            ProviderEvent::Finished(FinishReason::Stop),
        ]
    );
    let sent = server.await.unwrap();
    assert_eq!(sent[0].request_line, "POST /v1/responses HTTP/1.1");
    assert_eq!(
        sent[0].header("authorization"),
        Some(format!("Bearer {SENTINEL}").as_str())
    );
    assert!(!sent[0].request_line.contains(SENTINEL));
    let body = sent[0].body_json();
    assert_eq!(
        body["store"],
        json!(false),
        "OpenAI must not retain the conversation"
    );
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["reasoning"], json!({"effort": "low"}));
    assert_eq!(body["instructions"], "Reply in the user's language.");
}

#[tokio::test]
async fn openai_drops_the_reasoning_setting_for_a_model_that_refuses_it() {
    let (base, server) = server(vec![
        Reply::Status(
            400,
            vec![],
            r#"{"error":{"message":"Unsupported parameter: 'reasoning.effort' is not supported with this model.","type":"invalid_request_error","param":"reasoning.effort","code":"unsupported_parameter"}}"#.to_string(),
        ),
        Reply::Sse(vec![
            r#"{"type":"response.output_text.delta","delta":"ok"}"#,
            r#"{"type":"response.completed","response":{"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}"#,
        ]),
    ])
    .await;
    let (result, events) =
        collect(&client(ProviderKind::OpenAi, &base), &request("gpt-plain")).await;
    result.unwrap();
    assert_eq!(events[0], ProviderEvent::TextDelta("ok".into()));
    let sent = server.await.unwrap();
    assert!(sent[0].body_json().get("reasoning").is_some());
    assert!(
        sent[1].body_json().get("reasoning").is_none(),
        "asked again without it"
    );
}

#[tokio::test]
async fn openai_lists_text_models_newest_first() {
    let (base, server) = server(vec![Reply::Json(
        json!({"object": "list", "data": [
            {"id": "gpt-old", "object": "model", "created": 100, "owned_by": "openai"},
            {"id": "text-embedding-3-large", "object": "model", "created": 300, "owned_by": "openai"},
            {"id": "gpt-new", "object": "model", "created": 200, "owned_by": "openai"},
            {"id": "whisper-1", "object": "model", "created": 50, "owned_by": "openai"}
        ]})
        .to_string(),
    )])
    .await;
    let models = client(ProviderKind::OpenAi, &base)
        .list_models()
        .await
        .unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["gpt-new", "gpt-old"]);
    let sent = server.await.unwrap();
    assert_eq!(sent[0].request_line, "GET /v1/models HTTP/1.1");
}

#[tokio::test]
async fn anthropic_streams_and_sends_its_own_headers() {
    let (base, server) = server(vec![Reply::Sse(vec![
        r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"usage":{"input_tokens":9,"output_tokens":1}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Salut!"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":3}}"#,
        r#"{"type":"message_stop"}"#,
    ])])
    .await;
    let (result, events) = collect(
        &client(ProviderKind::Anthropic, &base),
        &request("claude-test"),
    )
    .await;
    result.unwrap();
    assert_eq!(
        events,
        vec![
            ProviderEvent::TextDelta("Salut!".into()),
            ProviderEvent::Usage(Usage {
                prompt_tokens: 9,
                output_tokens: 3,
                total_tokens: 12
            }),
            ProviderEvent::Finished(FinishReason::Stop),
        ]
    );
    let sent = server.await.unwrap();
    assert_eq!(sent[0].request_line, "POST /v1/messages HTTP/1.1");
    assert_eq!(sent[0].header("x-api-key"), Some(SENTINEL));
    assert_eq!(sent[0].header("anthropic-version"), Some("2023-06-01"));
    assert!(sent[0].header("authorization").is_none());
    let body = sent[0].body_json();
    assert_eq!(body["system"], "Reply in the user's language.");
    assert_eq!(body["messages"][0]["role"], "user");
}

#[tokio::test]
async fn deepseek_lists_its_models_and_answers_through_its_anthropic_endpoint() {
    let (base, server) = server(vec![
        Reply::Json(json!({"object": "list", "data": [{"id": "deepseek-flash", "object": "model", "owned_by": "deepseek"}]}).to_string()),
        Reply::Sse(vec![
            r#"{"type":"message_start","message":{"usage":{"input_tokens":5,"output_tokens":0}}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
            r#"{"type":"message_stop"}"#,
        ]),
    ])
    .await;
    let deepseek = client(ProviderKind::DeepSeek, &base);
    let models = deepseek.list_models().await.unwrap();
    assert_eq!(models[0].id, "deepseek-flash");
    let (result, _) = collect(&deepseek, &request("deepseek-flash")).await;
    result.unwrap();
    let sent = server.await.unwrap();
    assert_eq!(sent[0].request_line, "GET /models HTTP/1.1");
    assert_eq!(
        sent[0].header("authorization"),
        Some(format!("Bearer {SENTINEL}").as_str())
    );
    assert_eq!(sent[1].request_line, "POST /anthropic/v1/messages HTTP/1.1");
    assert_eq!(sent[1].header("x-api-key"), Some(SENTINEL));
}

#[tokio::test]
async fn overload_and_a_refused_key_map_to_stable_errors_without_echoing_the_key() {
    let (base, _server) = server(vec![
        Reply::Status(529, vec![], r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#.to_string()),
        Reply::Status(401, vec![], format!(r#"{{"type":"error","error":{{"type":"authentication_error","message":"invalid x-api-key {SENTINEL}"}}}}"#)),
        Reply::Status(503, vec![], r#"{"error":{"message":"The server is overloaded","type":"server_error"}}"#.to_string()),
    ])
    .await;
    let anthropic = client(ProviderKind::Anthropic, &base);
    let overloaded = collect(&anthropic, &request("claude-test"))
        .await
        .0
        .unwrap_err();
    assert!(overloaded.is_transient(), "{overloaded:?}");
    let refused = collect(&anthropic, &request("claude-test"))
        .await
        .0
        .unwrap_err();
    assert!(matches!(refused, ProviderError::Unauthenticated(_)));
    assert!(!refused.to_string().contains(SENTINEL));
    let openai = client(ProviderKind::OpenAi, &base);
    let unavailable = collect(&openai, &request("gpt-test")).await.0.unwrap_err();
    assert!(matches!(
        unavailable,
        ProviderError::Unavailable { status: 503, .. }
    ));
}

#[test]
fn every_provider_refuses_a_foreign_endpoint() {
    for kind in ProviderKind::ALL {
        let key = ApiKey::new(SENTINEL.to_string()).unwrap();
        assert!(
            Client::new(kind, key, "https://evil.example.com").is_err(),
            "{kind:?}"
        );
        let key = ApiKey::new(SENTINEL.to_string()).unwrap();
        assert!(
            Client::new(kind, key, nexus_providers::production_base_url(kind)).is_ok(),
            "{kind:?}"
        );
    }
}
