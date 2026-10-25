//! The user's approval policy through the real host (ADR-POLICY-001,
//! step 6): every tool the agent can use is listed with what it does and
//! sends; a tool turned off is not offered and is refused if asked for
//! anyway; the window can tighten but not loosen (that takes the native
//! Windows dialog); and the user's number of register writes per reply
//! decides when a reply asks.

mod support;

use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_protocol::{ApprovalPath, ErrorCode, PermissionView, RunKind, SetPermissionRequest};
use serde_json::{Value, json};
use support::TestHost;
use support::chat::{gemini, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};

const TOKEN: &str = "permissions-token";
const KEY: &str = "SENTINEL-gemini-key-permissions-5c21";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(20),
        retry_waits: Vec::new(),
        ..RuntimeConfig::default()
    }
}

async fn connect(client: &NexusClient) {
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
}

fn declared(body: &Value) -> Vec<String> {
    body["tools"][0]["functionDeclarations"]
        .as_array()
        .map(|t| {
            t.iter()
                .map(|d| d["name"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn find<'a>(all: &'a [PermissionView], tool: &str) -> &'a PermissionView {
    all.iter().find(|p| p.tool_id == tool).unwrap()
}

fn change(
    tool: &str,
    enabled: bool,
    max_auto: Option<u32>,
    via: ApprovalPath,
) -> SetPermissionRequest {
    SetPermissionRequest {
        tool_id: tool.into(),
        enabled,
        max_auto,
        via,
    }
}

#[tokio::test]
async fn every_tool_the_agent_can_use_is_listed_with_what_it_sends() {
    let host = TestHost::start(TOKEN).await;
    let client = host.client().await;
    let all = client.permissions().await.unwrap();
    let ids: Vec<&str> = all.iter().map(|p| p.tool_id.as_str()).collect();
    for tool in [
        "WebRead",
        "RecordWrite",
        "MemoryFact",
        "CreateDocument",
        "McpToolCall",
    ] {
        assert!(ids.contains(&tool), "{tool} is listed");
    }
    for internal in [
        "RecordWriteBatch",
        "UndoRecordChange",
        "DeleteCreatedDocument",
    ] {
        assert!(
            !ids.contains(&internal),
            "{internal} is not the model's to use"
        );
    }
    let web = find(&all, "WebRead");
    assert!(web.enabled && web.decision == "Auto" && web.changed_at_ms.is_none());
    assert!(web.sends.contains("AI provider"));
    let writes = find(&all, "RecordWrite");
    assert_eq!(
        (writes.max_auto, writes.default_max_auto),
        (Some(10), Some(10))
    );
    assert_eq!(find(&all, "CreateDocument").decision, "Ask");
    host.shutdown().await;
}

#[tokio::test]
async fn a_tool_turned_off_is_not_offered_and_is_refused_if_asked_for() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;

    // Turning it off tightens the policy: the window may do it.
    let all = client
        .set_permission(change("WebRead", false, None, ApprovalPath::Window))
        .await
        .unwrap();
    let web = find(&all, "WebRead");
    assert!(!web.enabled && web.changed_at_ms.is_some());

    fake.push(Reply::calls(&[(
        "fetch_url",
        json!({"url": "https://example.com/"}),
    )]));
    fake.push(Reply::text(&["I cannot read pages now."]));
    let sent = send(&client, None, "p1", "Read example.com").await;
    settled(&client, &sent).await;
    let first = fake.generations()[0].json();
    assert!(
        !declared(&first).contains(&"fetch_url".to_string()),
        "a tool turned off is not offered"
    );
    let second = fake.generations()[1].json();
    let answer = second["contents"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["parts"].as_array().unwrap())
        .find(|p| p["functionResponse"]["name"] == "fetch_url")
        .map(|p| p["functionResponse"]["response"].clone())
        .expect("the call is answered");
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .contains("turned this tool off"),
        "{answer}"
    );

    // Back on loosens it: refused from the window, accepted from the dialog.
    let from_window = client
        .set_permission(change("WebRead", true, None, ApprovalPath::Window))
        .await;
    assert_eq!(from_window.unwrap_err().code(), Some(ErrorCode::Forbidden));
    let all = client
        .set_permission(change("WebRead", true, None, ApprovalPath::TrustedDialog))
        .await
        .unwrap();
    let web = find(&all, "WebRead");
    assert!(
        web.enabled && web.changed_at_ms.is_none(),
        "back to the default"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn the_users_number_of_writes_decides_when_a_reply_asks() {
    let fake = FakeGemini::start(KEY).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    connect(&client).await;

    // Fewer writes without asking tightens; more loosens.
    client
        .set_permission(change("RecordWrite", true, Some(0), ApprovalPath::Window))
        .await
        .unwrap();
    let more = client
        .set_permission(change("RecordWrite", true, Some(20), ApprovalPath::Window))
        .await;
    assert_eq!(more.unwrap_err().code(), Some(ErrorCode::Forbidden));
    let too_many = client
        .set_permission(change(
            "RecordWrite",
            true,
            Some(26),
            ApprovalPath::TrustedDialog,
        ))
        .await;
    assert_eq!(
        too_many.unwrap_err().code(),
        Some(ErrorCode::InvalidRequest)
    );

    // With 0, the agent's very first write waits for the user.
    fake.push(Reply::calls(&[(
        "add_record",
        json!({"register": "job_applications", "values": {
            "company": "Acme", "role": "IT Support"}}),
    )]));
    fake.push(Reply::text(&["Saved."]));
    let sent = send(&client, None, "w1", "I applied at Acme for IT Support.").await;
    let reply = settled(&client, &sent).await;
    let batch = client
        .run(reply.proposed_run_id.expect("the write waits in a batch"))
        .await
        .unwrap();
    assert_eq!(batch.kind, RunKind::RecordBatch);
    assert_eq!(batch.title, "1 change in Job applications");
    host.shutdown().await;
}
