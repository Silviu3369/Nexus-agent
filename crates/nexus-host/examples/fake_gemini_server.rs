//! TEST FAKE — never shipped. The scripted Gemini stand-in that
//! `scripts/m2-ui-check.ps1` points a debug host at (`NEXUS_GEMINI_BASE_URL`).
//! It is the same server the host tests use; it accepts only the key given
//! on the command line, prints its base URL and serves until killed.

#[allow(dead_code)]
#[path = "../tests/support/fake_gemini.rs"]
mod fake_gemini;

use std::io::Write;

use fake_gemini::{FakeGemini, Reply, text_event};

#[tokio::main]
async fn main() {
    let key = std::env::args()
        .nth(1)
        .expect("usage: fake_gemini_server <accepted-test-key>");
    let fake = FakeGemini::start(&key).await;
    // Replies in the order the UI check sends its messages.
    fake.push(Reply::text(&["Hello! ", "I'm your Nexus assistant."]));
    fake.push(Reply::calls(&[(
        "propose_note",
        serde_json::json!({"title": "Chat plan", "body": "step 1 from chat"}),
    )]));
    fake.push(Reply::SseThenHang(vec![text_event(
        "Starting a long reply",
    )]));
    fake.push(Reply::Status {
        code: 429,
        headers: vec![("retry-after", "30".to_string())],
        body: "{}".to_string(),
    });
    println!("{}", fake.base_url);
    std::io::stdout().flush().expect("stdout");
    std::future::pending::<()>().await;
}
