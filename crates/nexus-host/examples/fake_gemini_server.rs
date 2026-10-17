//! TEST FAKE — never shipped. The scripted Gemini stand-in that
//! `scripts/m2-ui-check.ps1` points a debug host at (`NEXUS_GEMINI_BASE_URL`),
//! plus a stand-in MCP server for the same check (reached only by a debug
//! host started with `NEXUS_ALLOW_LOOPBACK_MCP=1`). They are the servers the
//! host tests use; each accepts only the key given on the command line. It
//! prints the Gemini base URL, then the MCP URL, and serves until killed.

#[allow(dead_code)]
#[path = "../tests/support/fake_gemini.rs"]
mod fake_gemini;
#[allow(dead_code)]
#[path = "../tests/support/fake_mcp.rs"]
mod fake_mcp;

use std::io::Write;

use fake_gemini::{FakeGemini, Reply, text_event};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: fake_gemini_server <accepted-test-key> <accepted-mcp-key>";
    let key = args.next().expect(usage);
    let mcp_key = args.next().expect(usage);
    let fake = FakeGemini::start(&key).await;
    let mcp = fake_mcp::FakeMcp::start(
        Some(&mcp_key),
        vec![
            fake_mcp::tool("docs_search", "Search the documentation", true),
            fake_mcp::tool("delete_page", "Delete a documentation page", false),
        ],
    )
    .await;
    mcp.answer(
        "docs_search",
        "CreateNamedPipeW creates an instance of a named pipe.",
    );
    mcp.answer("delete_page", "Page 7 deleted.");
    // Replies in the order the UI check sends its messages.
    fake.push(Reply::text(&["Hello! ", "I'm your Nexus assistant."]));
    fake.push(Reply::calls(&[(
        "propose_note",
        serde_json::json!({"title": "Chat plan", "body": "step 1 from chat"}),
    )]));
    // After the user approves it, the task continues with the outcome.
    fake.push(Reply::text(&["I saved the note Chat plan."]));
    fake.push(Reply::SseThenHang(vec![text_event(
        "Starting a long reply",
    )]));
    fake.push(Reply::Status {
        code: 429,
        headers: vec![("retry-after", "30".to_string())],
        body: "{}".to_string(),
    });
    // The dashboard command bar: a web read the host refuses (loopback),
    // then an answer — so the check sees a failed step and a reply.
    fake.push(Reply::calls(&[(
        "fetch_url",
        serde_json::json!({"url": "http://127.0.0.1:1/private"}),
    )]));
    fake.push(Reply::text(&["Done from the dashboard."]));
    // A temporary outage (Google's "high demand" 503), then an answer: the
    // host tries again and the check sees both tries as steps.
    fake.push(Reply::status(
        503,
        r#"{"error":{"code":503,"message":"This model is currently experiencing high demand.","status":"UNAVAILABLE"}}"#,
    ));
    fake.push(Reply::text(&["Recovered after a retry."]));
    // Memory: the model proposes a fact, then answers; after the user
    // approves it, a later task is sent with it.
    fake.push(Reply::calls(&[(
        "propose_memory",
        serde_json::json!({"content": "The user prefers short answers.", "scope": "personal"}),
    )]));
    fake.push(Reply::text(&[
        "Noted; I will remember it once you approve.",
    ]));
    fake.push(Reply::text(&["Short answer."]));
    // MCP: once the user enabled the read-only tool, a task calls it.
    fake.push(Reply::calls(&[(
        "mcp1_docs_search",
        serde_json::json!({"q": "named pipes"}),
    )]));
    fake.push(Reply::text(&["From the docs: use CreateNamedPipeW."]));
    // A tool that can change things: the call waits for the user's
    // approval; once approved and made, the task continues.
    fake.push(Reply::calls(&[(
        "mcp1_delete_page",
        serde_json::json!({"q": "page 7"}),
    )]));
    fake.push(Reply::text(&["Page 7 is deleted."]));
    println!("{}", fake.base_url);
    println!("{}", mcp.url);
    std::io::stdout().flush().expect("stdout");
    std::future::pending::<()>().await;
}
