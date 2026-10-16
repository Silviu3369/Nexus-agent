//! MCP client against a controlled local HTTP server: the modern stateless
//! revision, the fallback to the session era, the headers, and refusals.
//! No real server is contacted (see `real_microsoft_learn_server`, ignored).

use nexus_providers::ApiKey;
use nexus_providers::mcp::{LEGACY_VERSION, MODERN_VERSION, McpClient, McpError};
use serde_json::{Value, json};

mod common;
use common::{Captured, Reply, server};

const TOKEN: &str = "SENTINEL-mcp-token-41c7";

fn header<'a>(c: &'a Captured, name: &str) -> Option<&'a str> {
    c.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn body(c: &Captured) -> Value {
    serde_json::from_str(&c.body).unwrap()
}

fn tools_page(next: Option<&str>, names: &[&str]) -> String {
    let tools: Vec<Value> = names
        .iter()
        .map(|n| json!({"name": n, "description": format!("tool {n}"), "inputSchema": {"type": "object",
            "properties": {"q": {"type": "string"}, "region": {"type": "string", "x-mcp-header": "Region"}}},
            "annotations": {"readOnlyHint": true}}))
        .collect();
    let mut result = json!({"tools": tools});
    if let Some(n) = next {
        result["nextCursor"] = json!(n);
    }
    json!({"jsonrpc": "2.0", "id": 1, "result": result}).to_string()
}

#[tokio::test]
async fn a_modern_server_is_spoken_to_statelessly_with_mirrored_headers() {
    let page_two = tools_page(None, &["fetch"]).replace("\"id\":1", "\"id\":2");
    let call = json!({"jsonrpc": "2.0", "id": 3, "result": {"resultType": "complete",
        "content": [{"type": "text", "text": "Found it"}], "isError": false}})
    .to_string();
    let (base, server) = server(vec![
        Reply::Json(tools_page(Some("p2"), &["search", "bad name"])),
        Reply::Json(page_two),
        // The call answers on an SSE stream, with a progress note first.
        Reply::Sse(vec![
            r#"{"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":1}}"#,
            Box::leak(call.into_boxed_str()),
        ]),
    ])
    .await;
    let client = McpClient::new(
        &format!("{base}/mcp"),
        Some(ApiKey::new(TOKEN.into()).unwrap()),
        true,
    )
    .unwrap();
    let listing = client.list_tools().await.unwrap();
    assert_eq!(
        listing
            .tools
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["search", "fetch"]
    );
    assert_eq!(listing.rejected.len(), 1, "a name with a space is left out");

    let out = client
        .call_tool(
            &listing.tools[0],
            json!({"q": "named pipes", "region": "Ünïcode"}),
        )
        .await
        .unwrap();
    assert_eq!(out.text, "Found it");
    assert!(!out.is_error && !out.truncated);

    let sent = server.await.unwrap();
    assert_eq!(sent.len(), 3);
    let list = &sent[0];
    assert_eq!(header(list, "MCP-Protocol-Version"), Some(MODERN_VERSION));
    assert_eq!(header(list, "Mcp-Method"), Some("tools/list"));
    assert_eq!(
        header(list, "Accept"),
        Some("application/json, text/event-stream")
    );
    assert_eq!(
        body(list)["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
        MODERN_VERSION
    );
    assert_eq!(body(&sent[1])["params"]["cursor"], "p2");
    let call = &sent[2];
    assert_eq!(header(call, "Mcp-Method"), Some("tools/call"));
    assert_eq!(header(call, "Mcp-Name"), Some("search"));
    assert_eq!(
        header(call, "Mcp-Param-Region"),
        Some("=?base64?w5xuw69jb2Rl?="),
        "non-ASCII header values use the Base64 sentinel"
    );
    assert_eq!(body(call)["params"]["arguments"]["q"], "named pipes");
    for c in &sent {
        assert_eq!(
            header(c, "Authorization"),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert!(
            !c.request_line.contains(TOKEN) && !c.body.contains(TOKEN),
            "the key only travels in the header"
        );
        assert!(
            header(c, "Mcp-Session-Id").is_none(),
            "no sessions in the modern revision"
        );
    }
}

#[tokio::test]
async fn a_session_era_server_gets_initialize_then_its_session_and_a_renewal() {
    let init = |id: u64, session: &'static str| {
        Reply::Status(
            200,
            vec![
                ("content-type", "application/json"),
                ("mcp-session-id", session),
            ],
            json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": LEGACY_VERSION,
                "capabilities": {"tools": {}}, "serverInfo": {"name": "old", "version": "1"}}})
            .to_string(),
        )
    };
    let (base, server) = server(vec![
        // A modern request (id 1) means nothing to it.
        Reply::Status(400, vec![("content-type", "text/plain")], "Bad Request: missing session".into()),
        init(2, "s-1"),
        Reply::Status(202, vec![], String::new()),
        Reply::Json(tools_page(None, &["search"]).replace("\"id\":1", "\"id\":3")),
        // The session expired (call id 4): a new one is started (id 5) and
        // the call retried (id 6).
        Reply::Status(404, vec![], String::new()),
        init(5, "s-2"),
        Reply::Status(202, vec![], String::new()),
        Reply::Json(
            json!({"jsonrpc": "2.0", "id": 6, "result": {"content": [{"type": "text", "text": "ok"}]}})
                .to_string(),
        ),
    ])
    .await;
    let client = McpClient::new(&format!("{base}/mcp"), None, true).unwrap();
    let listing = client.list_tools().await.unwrap();
    assert_eq!(listing.tools.len(), 1);
    let out = client
        .call_tool(&listing.tools[0], json!({"q": "x"}))
        .await
        .unwrap();
    assert_eq!(out.text, "ok");
    let sent = server.await.unwrap();
    assert_eq!(body(&sent[1])["method"], "initialize");
    assert_eq!(body(&sent[1])["params"]["protocolVersion"], LEGACY_VERSION);
    assert_eq!(body(&sent[2])["method"], "notifications/initialized");
    assert_eq!(header(&sent[2], "Mcp-Session-Id"), Some("s-1"));
    assert_eq!(header(&sent[3], "Mcp-Session-Id"), Some("s-1"));
    assert_eq!(
        header(&sent[3], "MCP-Protocol-Version"),
        Some(LEGACY_VERSION)
    );
    assert!(
        body(&sent[3])["params"].get("_meta").is_none(),
        "session-era requests carry no modern metadata"
    );
    assert_eq!(
        body(&sent[5])["method"],
        "initialize",
        "an expired session is renewed once"
    );
    assert_eq!(header(&sent[7], "Mcp-Session-Id"), Some("s-2"));
}

#[tokio::test]
async fn an_answer_to_another_request_is_refused() {
    let (base, server) = server(vec![Reply::Json(
        tools_page(None, &["search"]).replace("\"id\":1", "\"id\":99"),
    )])
    .await;
    let client = McpClient::new(&format!("{base}/mcp"), None, true).unwrap();
    assert!(matches!(
        client.list_tools().await,
        Err(McpError::Protocol(_))
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn refusals_redirects_and_input_requests_are_errors() {
    let (base, _server) = server(vec![
        Reply::Status(401, vec![("content-type", "application/json")], "{}".into()),
        Reply::Status(
            302,
            vec![("location", "https://elsewhere.example/mcp")],
            String::new(),
        ),
        Reply::Json(tools_page(None, &["search"])),
        Reply::Json(
            json!({"jsonrpc": "2.0", "id": 2, "result": {"resultType": "input_required",
                "inputRequests": {"login": {"method": "elicitation/create"}}}})
            .to_string(),
        ),
    ])
    .await;
    let client = McpClient::new(&format!("{base}/mcp"), None, true).unwrap();
    assert_eq!(
        client.list_tools().await.unwrap_err(),
        McpError::Unauthenticated(401)
    );
    let redirected = client.list_tools().await.unwrap_err();
    assert!(
        !matches!(redirected, McpError::Unauthenticated(_)),
        "a redirect is not followed: {redirected:?}"
    );
    let client = McpClient::new(&format!("{base}/mcp"), None, true).unwrap();
    let listing = client.list_tools().await.unwrap();
    assert_eq!(
        client
            .call_tool(&listing.tools[0], json!({}))
            .await
            .unwrap_err(),
        McpError::InputRequired
    );
}

#[tokio::test]
async fn private_addresses_are_refused_outside_tests() {
    // Written as an address: refused before anything is sent.
    for url in [
        "https://127.0.0.1/mcp",
        "https://10.0.0.5/mcp",
        "https://[::1]/mcp",
    ] {
        assert!(
            matches!(
                McpClient::new(url, None, false),
                Err(McpError::DisallowedHost(_))
            ),
            "{url}"
        );
    }
    // A name is checked when it is resolved, before connecting.
    let client = McpClient::new("https://localhost/mcp", None, false).unwrap();
    assert!(matches!(
        client.list_tools().await,
        Err(McpError::DisallowedHost(_))
    ));
}

/// The real Microsoft Learn MCP server (public, read-only, no account): run
/// with `cargo test -p nexus-providers --test mcp_protocol -- --ignored`.
/// Sends one fixed documentation query; no user data.
#[tokio::test]
#[ignore = "contacts learn.microsoft.com"]
async fn real_microsoft_learn_server() {
    let client = McpClient::new("https://learn.microsoft.com/api/mcp", None, false).unwrap();
    let listing = client.list_tools().await.unwrap();
    let names: Vec<&str> = listing.tools.iter().map(|t| t.name.as_str()).collect();
    println!("tools: {names:?}; rejected: {:?}", listing.rejected);
    let search = listing
        .tools
        .iter()
        .find(|t| t.name == "microsoft_docs_search")
        .expect("the search tool is offered");
    assert_eq!(search.read_only_hint(), Some(true));
    let out = client
        .call_tool(search, json!({"query": "Windows named pipes security"}))
        .await
        .unwrap();
    println!(
        "{} chars, truncated: {}, error: {}",
        out.text.len(),
        out.truncated,
        out.is_error
    );
    println!("{}", out.text.chars().take(400).collect::<String>());
    assert!(!out.is_error && !out.text.is_empty());
}
