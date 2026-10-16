//! A stand-in MCP server on 127.0.0.1, for tests only. It speaks the modern
//! stateless revision over JSON: `tools/list` answers the current tool
//! definitions (a test can change them), `tools/call` answers a fixed text
//! per tool. Every request is recorded as received.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use super::fake_gemini::{Received, read_request, write};

struct State {
    token: Option<String>,
    tools: Mutex<Vec<Value>>,
    results: Mutex<HashMap<String, String>>,
    received: Mutex<Vec<Received>>,
}

pub struct FakeMcp {
    pub url: String,
    state: Arc<State>,
    accept: tokio::task::JoinHandle<()>,
}

/// A tool definition as a server lists it.
pub fn tool(name: &str, description: &str, read_only: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]},
        "annotations": {"readOnlyHint": read_only}
    })
}

impl FakeMcp {
    /// Requests without `Authorization: Bearer <token>` get 401 when a
    /// token is set.
    pub async fn start(token: Option<&str>, tools: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://127.0.0.1:{}/mcp",
            listener.local_addr().unwrap().port()
        );
        let state = Arc::new(State {
            token: token.map(str::to_string),
            tools: Mutex::new(tools),
            results: Mutex::new(HashMap::new()),
            received: Mutex::new(Vec::new()),
        });
        let shared = Arc::clone(&state);
        let accept = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(serve(socket, Arc::clone(&shared)));
            }
        });
        Self { url, state, accept }
    }

    pub fn set_tools(&self, tools: Vec<Value>) {
        *self.state.tools.lock().unwrap() = tools;
    }

    pub fn answer(&self, tool: &str, text: &str) {
        self.state
            .results
            .lock()
            .unwrap()
            .insert(tool.to_string(), text.to_string());
    }

    pub fn received(&self) -> Vec<Received> {
        self.state.received.lock().unwrap().clone()
    }

    /// The `tools/call` requests, in order.
    pub fn calls(&self) -> Vec<Value> {
        self.received()
            .iter()
            .map(Received::json)
            .filter(|b| b["method"] == "tools/call")
            .collect()
    }
}

impl Drop for FakeMcp {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

async fn serve(mut socket: TcpStream, state: Arc<State>) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    state.received.lock().unwrap().push(request.clone());
    let (code, body) = if state
        .token
        .as_ref()
        .is_some_and(|t| request.header("authorization") != Some(format!("Bearer {t}").as_str()))
    {
        (401, json!({"error": "unauthorized"}).to_string())
    } else {
        let rpc: Value = serde_json::from_str(&request.body).unwrap_or(Value::Null);
        let id = rpc["id"].clone();
        let result = match rpc["method"].as_str() {
            Some("tools/list") => json!({"tools": *state.tools.lock().unwrap()}),
            Some("tools/call") => {
                let name = rpc["params"]["name"].as_str().unwrap_or_default();
                let text = state
                    .results
                    .lock()
                    .unwrap()
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| format!("no answer set for {name}"));
                json!({"resultType": "complete", "content": [{"type": "text", "text": text}], "isError": false})
            }
            _ => {
                let error = json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": "Method not found"}});
                respond(&mut socket, 200, &error.to_string()).await;
                return;
            }
        };
        (
            200,
            json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
        )
    };
    respond(&mut socket, code, &body).await;
}

async fn respond(socket: &mut TcpStream, code: u16, body: &str) {
    write(
        socket,
        &format!(
            "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await;
    let _ = socket.shutdown().await;
}
