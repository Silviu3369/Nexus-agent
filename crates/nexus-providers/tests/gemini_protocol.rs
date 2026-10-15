//! Gemini adapter against a controlled local HTTP server (`PLAN_NEXUS_RO.md`
//! M2: "teste cu server controlat ... probe de protocol"). No real call is
//! made; the server records exactly what the adapter sent.

use std::time::Duration;

use nexus_providers::gemini::GeminiClient;
use nexus_providers::{
    ApiKey, FinishReason, Message, ProviderError, ProviderEvent, ProviderRequest, Role,
    ThinkingLevel, ToolDeclaration, Usage,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SENTINEL: &str = "SENTINEL-gemini-key-7f3a";

#[derive(Debug, Default)]
struct Captured {
    request_line: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn body_json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("the adapter sends JSON")
    }
}

enum Reply {
    /// Server-sent events, each written separately (with a short pause).
    Sse(Vec<&'static str>),
    /// Raw bytes written in separate writes, to split events mid-way.
    Raw(Vec<&'static str>),
    Status(u16, Vec<(&'static str, &'static str)>, String),
    Json(String),
    /// Accept and never answer.
    Stall,
}

/// Serves the given replies to successive connections; returns the base URL
/// and a handle yielding what each request contained.
async fn server(replies: Vec<Reply>) -> (String, tokio::task::JoinHandle<Vec<Captured>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let handle = tokio::spawn(async move {
        let mut captured = Vec::new();
        for reply in replies {
            let (mut socket, _) = listener.accept().await.unwrap();
            captured.push(read_request(&mut socket).await);
            match reply {
                Reply::Sse(events) => {
                    write(&mut socket, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n").await;
                    for e in events {
                        write(&mut socket, &format!("data: {e}\r\n\r\n")).await;
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
                Reply::Raw(parts) => {
                    write(&mut socket, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n").await;
                    for p in parts {
                        write(&mut socket, p).await;
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }
                Reply::Status(code, headers, body) => {
                    let mut head = format!(
                        "HTTP/1.1 {code} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                        body.len()
                    );
                    for (k, v) in headers {
                        head.push_str(&format!("{k}: {v}\r\n"));
                    }
                    write(&mut socket, &format!("{head}\r\n{body}")).await;
                }
                Reply::Json(body) => {
                    write(
                        &mut socket,
                        &format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()),
                    )
                    .await;
                }
                Reply::Stall => {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            }
            let _ = socket.shutdown().await;
        }
        captured
    });
    (base, handle)
}

async fn write(socket: &mut tokio::net::TcpStream, text: &str) {
    let _ = socket.write_all(text.as_bytes()).await;
    let _ = socket.flush().await;
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Captured {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = socket.read(&mut chunk).await.unwrap();
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        assert!(n > 0, "client closed before sending headers");
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.parse().unwrap())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await.unwrap();
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Captured {
        request_line,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

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
                name: "propose_note".into(),
                arguments: serde_json::json!({"title": "Plan"})
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
                name: "fetch_url".to_string(),
                arguments: serde_json::json!({"url": "https://example.com"}),
            }],
        },
        Message {
            role: Role::User,
            parts: vec![nexus_providers::Part::FunctionResponse {
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
