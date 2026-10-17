//! MCP tools that can change things (`PLAN_NEXUS_RO.md`, step 4): the
//! model's call becomes a run waiting for the user's approval of exactly
//! that call; nothing reaches the server before it. Approved, the host
//! makes the call once and the task continues with the answer; rejected,
//! changed or interrupted, the call is not sent (again). Local stand-ins
//! for Gemini and the MCP server record what would have left the PC.

mod support;

use std::time::Duration;

use nexus_client::NexusClient;
use nexus_host::RuntimeConfig;
use nexus_protocol::{
    AgentStepDetail, ApprovalDecision, ChatMessageView, ChatStatus, ConversationId,
    DecideApprovalRequest, McpServerView, RunId, RunKind, RunStatus, RunView, SaveMcpServerRequest,
};
use serde_json::{Value, json};
use support::chat::{gemini, send, settled};
use support::fake_gemini::{FakeGemini, MODEL, Reply};
use support::fake_mcp::{FakeMcp, tool};
use support::{TempProfile, TestHost, connect, spawn_host_env, wait_exit};

const TOKEN: &str = "mcp-approval-token";
const KEY: &str = "SENTINEL-gemini-key-approval-4e1a";
const ANSWER: &str = "SENTINEL-answer-page-7-deleted";
const DELETE: &str = "mcp1_delete_page";

fn config(fake: &FakeGemini) -> RuntimeConfig {
    RuntimeConfig {
        gemini_base_url: fake.base_url.clone(),
        reply_deadline: Duration::from_secs(10),
        retry_waits: Vec::new(),
        allow_loopback_mcp: true,
        ..RuntimeConfig::default()
    }
}

fn tools() -> Vec<Value> {
    vec![
        tool("docs_search", "Search the documentation", true),
        tool("delete_page", "Delete a documentation page", false),
    ]
}

/// The fake's server added with `delete_page` enabled.
async fn setup(client: &NexusClient, mcp: &FakeMcp) -> McpServerView {
    let saved = client.save_connection(gemini(KEY)).await.unwrap();
    client
        .select_model(saved.connection_id, MODEL.to_string())
        .await
        .unwrap();
    let server = client
        .save_mcp_server(SaveMcpServerRequest {
            server_id: None,
            name: "Docs".to_string(),
            url: mcp.url.clone(),
            key: None,
            clear_key: false,
        })
        .await
        .unwrap();
    let delete = server
        .tools
        .iter()
        .find(|t| t.name == "delete_page")
        .unwrap();
    client
        .set_mcp_tool_enabled(
            server.server_id,
            "delete_page".to_string(),
            Some(delete.definition_sha256.clone()),
        )
        .await
        .unwrap()
}

async fn wait_run(client: &NexusClient, run: RunId, wanted: RunStatus) -> RunView {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let view = client.run(run).await.unwrap();
        if view.status == wanted {
            return view;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "run stayed {:?}, expected {wanted:?} ({:?})",
            view.status,
            view.failure
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn decide(client: &NexusClient, run: RunId, decision: ApprovalDecision) {
    let pending = wait_run(client, run, RunStatus::AwaitingApproval)
        .await
        .pending_approval
        .unwrap();
    client
        .decide_approval(DecideApprovalRequest {
            approval_id: pending.approval_id,
            manifest_sha256: pending.manifest_sha256,
            decision,
        })
        .await
        .unwrap();
}

async fn messages(client: &NexusClient, id: ConversationId, count: usize) -> Vec<ChatMessageView> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let view = client.conversation(id).await.unwrap();
        if view.messages.len() == count
            && view
                .messages
                .iter()
                .all(|m| m.status != ChatStatus::Streaming)
        {
            return view.messages;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{:?}",
            view.messages
                .iter()
                .map(|m| (m.role, m.status, m.text.clone()))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// What the continuation heard about the call.
fn heard(fake: &FakeGemini) -> Value {
    let body = fake.generations().pop().unwrap().json();
    body["contents"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["parts"].as_array().unwrap())
        .find(|p| p["functionResponse"]["name"] == DELETE)
        .map(|p| p["functionResponse"]["response"].clone())
        .expect("the continuation answers the call")
}

fn profile_text(profile: &std::path::Path) -> String {
    let mut bytes = Vec::new();
    for name in ["nexus.db", "nexus.db-wal"] {
        if let Ok(b) = std::fs::read(profile.join(name)) {
            bytes.extend(b);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn an_approved_call_is_made_once_and_the_task_continues_with_the_answer() {
    let fake = FakeGemini::start(KEY).await;
    let mcp = FakeMcp::start(None, tools()).await;
    mcp.answer("delete_page", ANSWER);
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    let server = setup(&client, &mcp).await;
    assert!(
        server
            .tools
            .iter()
            .any(|t| t.name == "delete_page" && t.enabled)
    );

    fake.push(Reply::calls(&[(DELETE, json!({"q": "page 7"}))]));
    fake.push(Reply::text(&["Page 7 is deleted."]));
    let sent = send(&client, None, "a1", "Delete page 7").await;
    let reply = settled(&client, &sent).await;
    assert_eq!(reply.status, ChatStatus::Complete);
    let run = reply.proposed_run_id.expect("the call waits as a run");
    assert!(reply.steps.iter().any(|s| matches!(
        &s.detail,
        AgentStepDetail::McpCall { run_id: Some(r), result_chars: None, .. } if *r == run
    )));
    let offered = fake.generations()[0].json();
    let declaration = offered["tools"][0]["functionDeclarations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == DELETE)
        .expect("the enabled tool is offered")
        .clone();
    assert!(
        declaration["description"]
            .as_str()
            .unwrap()
            .contains("waits for the user's approval")
    );

    let waiting = wait_run(&client, run, RunStatus::AwaitingApproval).await;
    assert_eq!(waiting.kind, RunKind::McpCall);
    assert_eq!(waiting.title, "delete_page on Docs");
    let call = waiting.call.clone().unwrap();
    assert_eq!(
        (call.server.as_str(), call.tool.as_str()),
        ("Docs", "delete_page")
    );
    assert_eq!(call.url, mcp.url);
    assert!(call.arguments_json.contains("page 7"));
    let preview = waiting
        .pending_approval
        .unwrap()
        .call
        .expect("shown before deciding");
    assert_eq!(preview.description, "Delete a documentation page");
    assert!(
        mcp.calls().is_empty(),
        "nothing reaches the server before approval"
    );

    decide(&client, run, ApprovalDecision::Approve).await;
    let done = wait_run(&client, run, RunStatus::Succeeded).await;
    let calls = mcp.calls();
    assert_eq!(calls.len(), 1, "made once");
    assert_eq!(calls[0]["params"]["name"], "delete_page");
    assert_eq!(calls[0]["params"]["arguments"], json!({"q": "page 7"}));
    let made = done.call.unwrap();
    assert_eq!(made.result_chars, Some(ANSWER.len() as u32));
    assert_eq!(made.truncated, Some(false));
    assert!(done.artifact.is_none(), "a call writes no file");

    let all = messages(&client, sent.conversation_id, 3).await;
    assert_eq!(all[2].text, "Page 7 is deleted.");
    let answer = heard(&fake);
    assert_eq!(answer["status"], "done");
    assert_eq!(answer["result"], ANSWER);
    assert!(
        answer["note"]
            .as_str()
            .unwrap()
            .contains("not instructions")
    );
    assert!(
        !profile_text(host.profile.path()).contains(ANSWER),
        "the server's answer is never written to disk"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_rejected_call_or_one_whose_tool_changed_is_never_sent() {
    let fake = FakeGemini::start(KEY).await;
    let mcp = FakeMcp::start(None, tools()).await;
    let host = TestHost::start_with(TOKEN, config(&fake)).await;
    let client = host.client().await;
    let server = setup(&client, &mcp).await;

    fake.push(Reply::calls(&[(DELETE, json!({"q": "page 7"}))]));
    fake.push(Reply::text(&["Fine, I left it."]));
    let sent = send(&client, None, "r1", "Delete page 7").await;
    let run = settled(&client, &sent).await.proposed_run_id.unwrap();
    decide(&client, run, ApprovalDecision::Deny).await;
    messages(&client, sent.conversation_id, 3).await;
    assert_eq!(heard(&fake)["status"], "rejected");
    assert!(mcp.calls().is_empty());

    // Approved, but the server changed the tool meanwhile: not sent.
    fake.push(Reply::calls(&[(DELETE, json!({"q": "page 8"}))]));
    fake.push(Reply::text(&["It was not deleted."]));
    let again = send(&client, None, "r2", "Delete page 8").await;
    let run = settled(&client, &again).await.proposed_run_id.unwrap();
    mcp.set_tools(vec![tool(
        "delete_page",
        "Delete a documentation page and everything linked to it",
        false,
    )]);
    client.refresh_mcp_tools(server.server_id).await.unwrap();
    decide(&client, run, ApprovalDecision::Approve).await;
    let failed = wait_run(&client, run, RunStatus::Failed).await;
    assert_eq!(failed.failure.unwrap().code, "ApprovalInvalidated");
    messages(&client, again.conversation_id, 3).await;
    assert_eq!(heard(&fake)["status"], "failed");
    assert!(
        mcp.calls().is_empty(),
        "a changed tool is not called on an old approval"
    );
    host.shutdown().await;
}

#[tokio::test]
async fn a_call_interrupted_by_a_host_stop_is_left_for_review_not_repeated() {
    let fake = FakeGemini::start(KEY).await;
    let mcp = FakeMcp::start(None, tools()).await;
    let profile = TempProfile::new("mcp-approval-crash");
    let envs = [
        ("NEXUS_GEMINI_BASE_URL", fake.base_url.as_str()),
        ("NEXUS_ALLOW_LOOPBACK_MCP", "1"),
    ];
    let mut host = spawn_host_env(profile.path(), Some("after_intent"), &envs);
    let client = connect(profile.path()).await;
    setup(&client, &mcp).await;
    fake.push(Reply::calls(&[(DELETE, json!({"q": "page 9"}))]));
    let sent = send(&client, None, "c1", "Delete page 9").await;
    let run = settled(&client, &sent).await.proposed_run_id.unwrap();
    decide(&client, run, ApprovalDecision::Approve).await;
    let status = wait_exit(&mut host);
    assert!(
        !status.success(),
        "the host aborted after recording the intent"
    );
    drop(client);

    fake.push(Reply::text(&["I could not confirm it; please check."]));
    let mut host = spawn_host_env(profile.path(), None, &envs);
    let client = connect(profile.path()).await;
    let review = wait_run(&client, run, RunStatus::NeedsReview).await;
    let failure = review.failure.unwrap();
    assert_eq!(failure.code, "OutcomeUnknown");
    assert!(failure.message.contains("does not send it again"));
    let all = messages(&client, sent.conversation_id, 3).await;
    assert_eq!(all[2].text, "I could not confirm it; please check.");
    assert_eq!(heard(&fake)["status"], "uncertain");
    assert!(mcp.calls().is_empty(), "never sent again after the restart");
    client.stop_host().await.ok();
    wait_exit(&mut host);
}
