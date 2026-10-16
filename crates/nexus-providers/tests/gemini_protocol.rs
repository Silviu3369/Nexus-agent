//! Gemini adapter against a controlled local HTTP server (`PLAN_NEXUS_RO.md`
//! M2: "teste cu server controlat ... probe de protocol"). No real call is
//! made; the server records exactly what the adapter sent.

use std::time::Duration;

use nexus_providers::gemini::GeminiClient;
use nexus_providers::{
    ApiKey, FinishReason, Message, ProviderError, ProviderEvent, ProviderRequest, Role,
    ThinkingLevel, ToolDeclaration, Usage,
};

mod common;
use common::{Reply, server};

const SENTINEL: &str = "SENTINEL-gemini-key-7f3a";

fn client(base: &str) -> GeminiClient {
    GeminiClient::new(ApiKey::new(SENTINEL.to_string()).unwrap(), base).unwrap()
}

fn request() -> ProviderRequest {
    ProviderRequest {
        model: "gemini-test".to_string(),
        system_instruction: "Răspunde în limba utilizatorului.".to_string(),
        messages: vec![Message::text(Role::User, "Salut")],
        tools: vec![ToolDeclaration {
            name: "propose_note".to_string(),
            description: "Propune o notă".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {"title": {"type": "string"}}}),
        }],
        max_output_tokens: 256,
        deadline: Duration::from_secs(5),
        thinking_level: None,
    }
}

async fn collect(
    client: &GeminiClient,
    req: &ProviderRequest,
) -> (Result<(), ProviderError>, Vec<ProviderEvent>) {
    let mut events = Vec::new();
    let result = client.stream(req, |e| events.push(e)).await;
    (result, events)
}

#[tokio::test]
async fn streams_text_usage_and_finish_with_the_key_only_in_the_header() {
    let (base, server) = server(vec![Reply::Sse(vec![
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"Bună"}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":" ziua"}]}}]}"#,
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"!"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":3,"totalTokenCount":10}}"#,
    ])])
    .await;
    let (result, events) = collect(&client(&base), &request()).await;
    result.unwrap();
    assert_eq!(
        events,
        vec![
            ProviderEvent::TextDelta("Bună".into()),
            ProviderEvent::TextDelta(" ziua".into()),
            ProviderEvent::TextDelta("!".into()),
            ProviderEvent::Usage(Usage {
                prompt_tokens: 7,
                output_tokens: 3,
                total_tokens: 10
            }),
            ProviderEvent::Finished(FinishReason::Stop),
        ]
    );

    let sent = server.await.unwrap().remove(0);
    assert_eq!(
        sent.request_line,
        "POST /v1beta/models/gemini-test:streamGenerateContent?alt=sse HTTP/1.1"
    );
    assert_eq!(sent.header("x-goog-api-key"), Some(SENTINEL));
    assert!(
        !sent.request_line.contains(SENTINEL),
        "the key must never be in the URL"
    );
    let body: serde_json::Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(body["contents"][0]["role"], "user");
    assert_eq!(body["contents"][0]["parts"][0]["text"], "Salut");
    assert_eq!(
        body["systemInstruction"]["parts"][0]["text"],
        "Răspunde în limba utilizatorului."
    );
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["name"],
        "propose_note"
    );
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 256);
    assert!(
        !sent.body.contains(SENTINEL),
        "the key must never be in the body"
    );
}

#[tokio::test]
async fn function_call_is_reported_as_a_proposal_and_thoughts_are_skipped() {
    let (base, _server) = server(vec![Reply::Sse(vec![
        r#"{"candidates":[{"content":{"parts":[{"text":"internal reasoning","thought":true},{"functionCall":{"name":"propose_note","args":{"title":"Plan"}}}]},"finishReason":"STOP"}]}"#,
    ])])
    .await;
    let (result, events) = collect(&client(&base), &request()).await;
    result.unwrap();
    assert_eq!(
        events,
        vec![
            ProviderEvent::ToolProposal {
                id: None,
                name: "propose_note".into(),
                arguments: serde_json::json!({"title": "Plan"}),
                signature: None,
            },
            ProviderEvent::Finished(FinishReason::Stop),
        ]
    );
}

#[tokio::test]
async fn thinking_level_is_sent_for_gemini_3_models_and_only_when_chosen() {
    // Verified against ai.google.dev/gemini-api/docs/thinking (2026-10-15):
    // Gemini 3.x thinks by default (e.g. gemini-3.8-flash: "medium"), real
    // latency a short reply does not need; the user picks the level, which
    // only makes sense for the models that support it.
    let reply = || {
        Reply::Sse(vec![
            r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}]}"#,
        ])
    };
    let (base, server) = server(vec![reply(), reply(), reply()]).await;

    let mut high = request();
    high.model = "gemini-3.8-flash".to_string();
    high.thinking_level = Some(ThinkingLevel::High);
    collect(&client(&base), &high).await.0.unwrap();

    let mut unset = request();
    unset.model = "gemini-3.8-flash".to_string();
    unset.thinking_level = None;
    collect(&client(&base), &unset).await.0.unwrap();

    let mut legacy = request();
    legacy.model = "gemini-2.5-flash".to_string();
    legacy.thinking_level = Some(ThinkingLevel::Low);
    collect(&client(&base), &legacy).await.0.unwrap();

    let sent = server.await.unwrap();
    assert_eq!(
        sent[0].body_json()["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "HIGH"
    );
    assert!(
        sent[1].body_json()["generationConfig"]
            .get("thinkingConfig")
            .is_none(),
        "no choice made: the field is left out, Google's own default applies"
    );
    assert!(
        sent[2].body_json()["generationConfig"]
            .get("thinkingConfig")
            .is_none(),
        "an older model must not get a field verified only for Gemini 3.x: {}",
        sent[2].body
    );
}

#[tokio::test]
async fn a_tool_result_round_trips_in_the_shape_gemini_expects() {
    // Verified against ai.google.dev/api/generate-content (2026-10-15):
    // Content.role is "user" or "model" only ("function" is not a valid
    // value there, despite one contradictory mention elsewhere on the
    // site); a functionResponse carries "name" and "response" as siblings.
    let (base, server) = server(vec![Reply::Sse(vec![
        r#"{"candidates":[{"content":{"parts":[{"text":"Am citit pagina."}]},"finishReason":"STOP"}]}"#,
    ])])
    .await;
    let mut req = request();
    req.messages = vec![
        Message::text(Role::User, "Ce scrie pe pagina asta?"),
        Message {
            role: Role::Model,
            parts: vec![nexus_providers::Part::FunctionCall {
                id: None,
                name: "fetch_url".to_string(),
                arguments: serde_json::json!({"url": "https://example.com"}),
                signature: Some("SIG-fetch".to_string()),
            }],
        },
        Message {
            role: Role::User,
            parts: vec![nexus_providers::Part::FunctionResponse {
                id: None,
                name: "fetch_url".to_string(),
                response: serde_json::json!({"text": "Hello world", "status": 200}),
            }],
        },
    ];
    let (result, events) = collect(&client(&base), &req).await;
    result.unwrap();
    assert_eq!(
        events,
        vec![
            ProviderEvent::TextDelta("Am citit pagina.".into()),
            ProviderEvent::Finished(FinishReason::Stop)
        ]
    );

    let sent = server.await.unwrap().remove(0);
    let body: serde_json::Value = serde_json::from_str(&sent.body).unwrap();
    assert_eq!(body["contents"][0]["role"], "user");
    assert_eq!(body["contents"][1]["role"], "model");
    assert_eq!(
        body["contents"][1]["parts"][0]["functionCall"]["name"],
        "fetch_url"
    );
    assert_eq!(
        body["contents"][1]["parts"][0]["functionCall"]["args"]["url"],
        "https://example.com"
    );
    assert_eq!(body["contents"][2]["role"], "user");
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"]["name"],
        "fetch_url"
    );
    assert_eq!(
        body["contents"][2]["parts"][0]["functionResponse"]["response"]["text"],
        "Hello world"
    );
    assert_eq!(
        body["contents"][1]["parts"][0]["thoughtSignature"], "SIG-fetch",
        "the signature goes back on the call it came with"
    );
}

/// Gemini 3 rejects (400) a tool round trip whose first call in a step lacks
/// the thought signature it was given (ai.google.dev, thought signatures,
/// checked 2026-10-16). The signature is read from the call's part and sent
/// back on it; a first call the model did not sign gets Google's documented
/// value instead, and later calls are left as they are.
#[tokio::test]
async fn thought_signatures_are_kept_on_calls_and_unsigned_first_calls_get_the_documented_value() {
    let (base, server) = server(vec![
        Reply::Sse(vec![
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"fetch_url","args":{"url":"https://a"}},"thoughtSignature":"SIG-A"},{"functionCall":{"name":"fetch_url","args":{"url":"https://b"}}}]},"finishReason":"STOP"}]}"#,
        ]),
        Reply::Sse(vec![
            r#"{"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}]}"#,
        ]),
    ])
    .await;
    let (result, events) = collect(&client(&base), &request()).await;
    result.unwrap();
    assert_eq!(
        events[0],
        ProviderEvent::ToolProposal {
            id: None,
            name: "fetch_url".into(),
            arguments: serde_json::json!({"url": "https://a"}),
            signature: Some("SIG-A".into()),
        }
    );
    assert!(matches!(
        &events[1],
        ProviderEvent::ToolProposal {
            signature: None,
            ..
        }
    ));

    let call = |url: &str, signature: Option<&str>| nexus_providers::Part::FunctionCall {
        id: None,
        name: "fetch_url".to_string(),
        arguments: serde_json::json!({ "url": url }),
        signature: signature.map(str::to_string),
    };
    let answer = |url: &str| nexus_providers::Part::FunctionResponse {
        id: None,
        name: "fetch_url".to_string(),
        response: serde_json::json!({ "url": url }),
    };
    let mut req = request();
    req.model = "gemini-3.8-flash".to_string();
    req.messages = vec![
        Message::text(Role::User, "read both"),
        // Signed by the model: kept as is, on the first call only.
        Message {
            role: Role::Model,
            parts: vec![call("https://a", Some("SIG-A")), call("https://b", None)],
        },
        Message {
            role: Role::User,
            parts: vec![answer("https://a"), answer("https://b")],
        },
        // Unsigned (its signed sibling was not sent back): documented value.
        Message {
            role: Role::Model,
            parts: vec![call("https://c", None), call("https://d", None)],
        },
        Message {
            role: Role::User,
            parts: vec![answer("https://c"), answer("https://d")],
        },
    ];
    let (result, _) = collect(&client(&base), &req).await;
    result.unwrap();
    let sent = server.await.unwrap().remove(1);
    let body: serde_json::Value = serde_json::from_str(&sent.body).unwrap();
    let parts = |i: usize| body["contents"][i]["parts"].clone();
    assert_eq!(parts(1)[0]["thoughtSignature"], "SIG-A");
    assert!(
        parts(1)[1].get("thoughtSignature").is_none(),
        "parallel calls after the first carry none"
    );
    assert_eq!(
        parts(3)[0]["thoughtSignature"],
        "skip_thought_signature_validator"
    );
    assert!(parts(3)[1].get("thoughtSignature").is_none());
    assert!(
        parts(2)[0].get("thoughtSignature").is_none(),
        "responses never carry one"
    );
}

#[tokio::test]
async fn events_split_mid_json_across_writes_are_reassembled() {
    let (base, _server) = server(vec![Reply::Raw(vec![
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"te",
        "xt\":\"întreg\"}]},\"finishReason\":\"STOP\"}]}\r",
        "\n\r\n",
    ])])
    .await;
    let (result, events) = collect(&client(&base), &request()).await;
    result.unwrap();
    assert_eq!(events[0], ProviderEvent::TextDelta("întreg".into()));
}

#[tokio::test]
async fn unauthorized_is_typed_and_never_echoes_the_key() {
    let body = format!(
        r#"{{"error":{{"code":401,"message":"API key {SENTINEL} not valid","status":"UNAUTHENTICATED"}}}}"#
    );
    let (base, _server) = server(vec![Reply::Status(401, vec![], body)]).await;
    let (result, events) = collect(&client(&base), &request()).await;
    assert!(events.is_empty());
    match result {
        Err(ProviderError::Unauthenticated(message)) => {
            assert!(
                !message.contains(SENTINEL),
                "the key leaked into: {message}"
            );
            assert!(message.contains("<redacted>"));
        }
        other => panic!("expected Unauthenticated, got {other:?}"),
    }
}

#[tokio::test]
async fn a_malformed_key_is_unauthenticated_even_though_google_answers_400() {
    // Real Google behaviour (verified against the live API, not a guess):
    // an invalid key gets HTTP 400 INVALID_ARGUMENT, with the reason in a
    // structured ErrorInfo detail, not a 401.
    let body = format!(
        r#"{{"error":{{"code":400,"message":"API key {SENTINEL} not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_INVALID","domain":"googleapis.com"}},{{"@type":"type.googleapis.com/google.rpc.LocalizedMessage","locale":"en-US","message":"API key not valid."}}]}}}}"#
    );
    let (base, _server) = server(vec![Reply::Status(400, vec![], body)]).await;
    let (result, _) = collect(&client(&base), &request()).await;
    match result {
        Err(ProviderError::Unauthenticated(message)) => {
            assert!(
                !message.contains(SENTINEL),
                "the key leaked into: {message}"
            );
        }
        other => panic!("expected Unauthenticated, got {other:?}"),
    }
}

#[tokio::test]
async fn a_400_without_the_api_key_reason_stays_a_plain_rejection() {
    let (base, _server) = server(vec![Reply::Status(
        400,
        vec![],
        r#"{"error":{"code":400,"message":"bad field","status":"INVALID_ARGUMENT"}}"#.into(),
    )])
    .await;
    let (result, _) = collect(&client(&base), &request()).await;
    assert_eq!(
        result,
        Err(ProviderError::Rejected {
            status: 400,
            message: "bad field".into()
        })
    );
}

#[tokio::test]
async fn rate_limit_carries_retry_after() {
    let (base, _server) = server(vec![Reply::Status(
        429,
        vec![("retry-after", "17")],
        "{}".into(),
    )])
    .await;
    let (result, _) = collect(&client(&base), &request()).await;
    assert_eq!(
        result,
        Err(ProviderError::RateLimited {
            retry_after: Some(Duration::from_secs(17))
        })
    );
}

#[tokio::test]
async fn bad_request_and_server_errors_are_distinguished() {
    let (base, _server) = server(vec![
        Reply::Status(400, vec![], r#"{"error":{"message":"bad field"}}"#.into()),
        Reply::Status(503, vec![], r#"{"error":{"message":"overloaded"}}"#.into()),
    ])
    .await;
    let c = client(&base);
    assert_eq!(
        collect(&c, &request()).await.0,
        Err(ProviderError::Rejected {
            status: 400,
            message: "bad field".into()
        })
    );
    assert_eq!(
        collect(&c, &request()).await.0,
        Err(ProviderError::Unavailable {
            status: 503,
            message: "overloaded".into()
        })
    );
}

#[tokio::test]
async fn malformed_chunk_is_an_invalid_response() {
    let (base, _server) = server(vec![Reply::Sse(vec!["{not json"])]).await;
    let (result, _) = collect(&client(&base), &request()).await;
    assert!(
        matches!(result, Err(ProviderError::InvalidResponse(_))),
        "{result:?}"
    );
}

#[tokio::test]
async fn blocked_prompt_is_reported() {
    let (base, _server) = server(vec![Reply::Sse(vec![
        r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#,
    ])])
    .await;
    let (result, _) = collect(&client(&base), &request()).await;
    assert_eq!(result, Err(ProviderError::Blocked("SAFETY".into())));
}

#[tokio::test]
async fn a_stalled_provider_hits_the_deadline() {
    let (base, _server) = server(vec![Reply::Stall]).await;
    let mut req = request();
    req.deadline = Duration::from_millis(300);
    let started = std::time::Instant::now();
    let (result, _) = collect(&client(&base), &req).await;
    assert_eq!(result, Err(ProviderError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn model_list_is_paginated_filtered_and_authenticated_by_header() {
    let (base, server) = server(vec![
        Reply::Json(
            r#"{"models":[{"name":"models/gemini-a","displayName":"A","inputTokenLimit":1000,"outputTokenLimit":100,"supportedGenerationMethods":["generateContent"]},{"name":"models/embed-x","supportedGenerationMethods":["embedContent"]}],"nextPageToken":"p2"}"#.into(),
        ),
        Reply::Json(r#"{"models":[{"name":"models/gemini-b","supportedGenerationMethods":["generateContent","countTokens"]}]}"#.into()),
    ])
    .await;
    let models = client(&base).list_models().await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        ["gemini-a", "gemini-b"],
        "embedding-only models are filtered out"
    );
    assert_eq!(models[0].input_token_limit, Some(1000));

    let sent = server.await.unwrap();
    assert!(
        sent[0]
            .request_line
            .starts_with("GET /v1beta/models?pageSize=200 ")
    );
    assert!(sent[1].request_line.contains("pageToken=p2"));
    for request in &sent {
        assert_eq!(request.header("x-goog-api-key"), Some(SENTINEL));
        assert!(!request.request_line.contains(SENTINEL));
    }
}

#[tokio::test]
async fn redirects_are_not_followed() {
    // A redirect could carry the key header to another origin.
    let (base, _server) = server(vec![Reply::Status(
        302,
        vec![("location", "http://127.0.0.1:9/steal")],
        String::new(),
    )])
    .await;
    let (result, _) = collect(&client(&base), &request()).await;
    assert!(
        matches!(result, Err(ProviderError::Unavailable { status: 302, .. })),
        "{result:?}"
    );
}
