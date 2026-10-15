//! A programmable stand-in for the Gemini API on 127.0.0.1, for tests only.
//! The model list is answered from a fixed catalogue; each generation request
//! takes the next queued reply. Every request is recorded as received, so a
//! test can check exactly what would have left the PC.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const MODEL: &str = "gemini-fake-flash";

pub enum Reply {
    /// Server-sent events with a pause between them, then the stream ends.
    Sse(Vec<String>),
    /// These events, then the stream stays open without finishing.
    SseThenHang(Vec<String>),
    Status {
        code: u16,
        headers: Vec<(&'static str, String)>,
        body: String,
    },
    /// Accepts the request and never answers.
    Hang,
}

impl Reply {
    /// A complete text reply split in `parts`, with usage on the last event.
    pub fn text(parts: &[&str]) -> Self {
        let mut events: Vec<String> = parts.iter().map(|p| text_event(p)).collect();
        events.push(finish_event(11, 5));
        Reply::Sse(events)
    }

    /// One event carrying these function calls, then a normal finish.
    pub fn calls(calls: &[(&str, serde_json::Value)]) -> Self {
        let parts: Vec<serde_json::Value> = calls
            .iter()
            .map(|(name, args)| serde_json::json!({"functionCall": {"name": name, "args": args}}))
            .collect();
        let event =
            serde_json::json!({"candidates": [{"content": {"role": "model", "parts": parts}}]});
        Reply::Sse(vec![event.to_string(), finish_event(9, 4)])
    }

    pub fn status(code: u16, body: &str) -> Self {
        Reply::Status {
            code,
            headers: Vec::new(),
            body: body.to_string(),
        }
    }
}

pub fn text_event(text: &str) -> String {
    serde_json::json!({"candidates": [{"content": {"role": "model", "parts": [{"text": text}]}}]})
        .to_string()
}

fn finish_event(prompt: u32, output: u32) -> String {
    serde_json::json!({
        "candidates": [{"content": {"role": "model", "parts": []}, "finishReason": "STOP"}],
        "usageMetadata": {"promptTokenCount": prompt, "candidatesTokenCount": output, "totalTokenCount": prompt + output}
    })
    .to_string()
}

#[derive(Debug, Clone)]
pub struct Received {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Received {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn is_generation(&self) -> bool {
        self.method == "POST" && self.target.contains(":streamGenerateContent")
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("the adapter sends JSON")
    }
}

struct State {
    valid_key: String,
    replies: Mutex<VecDeque<Reply>>,
    received: Mutex<Vec<Received>>,
}

pub struct FakeGemini {
    pub base_url: String,
    state: Arc<State>,
    accept: tokio::task::JoinHandle<()>,
}

impl FakeGemini {
    /// Accepts only `valid_key`; any other key gets Google's 401 shape,
    /// which echoes the presented key (so redaction is exercised).
    pub async fn start(valid_key: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let state = Arc::new(State {
            valid_key: valid_key.to_string(),
            replies: Mutex::new(VecDeque::new()),
            received: Mutex::new(Vec::new()),
        });
        let shared = Arc::clone(&state);
        let accept = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(serve(socket, Arc::clone(&shared)));
            }
        });
        Self {
            base_url,
            state,
            accept,
        }
    }

    pub fn push(&self, reply: Reply) {
        self.state.replies.lock().unwrap().push_back(reply);
    }

    pub fn received(&self) -> Vec<Received> {
        self.state.received.lock().unwrap().clone()
    }

    pub fn generations(&self) -> Vec<Received> {
        self.received()
            .into_iter()
            .filter(Received::is_generation)
            .collect()
    }
}

impl Drop for FakeGemini {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

async fn serve(mut socket: TcpStream, state: Arc<State>) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    state.received.lock().unwrap().push(request.clone());

    let key = request
        .header("x-goog-api-key")
        .unwrap_or_default()
        .to_string();
    let reply = if key != state.valid_key {
        Reply::status(
            401,
            &format!(
                r#"{{"error":{{"code":401,"message":"API key not valid: {key}","status":"UNAUTHENTICATED"}}}}"#
            ),
        )
    } else if request.method == "GET" && request.target.starts_with("/v1beta/models") {
        Reply::Status {
            code: 200,
            headers: vec![("content-type", "application/json".to_string())],
            body: serde_json::json!({"models": [
                {"name": format!("models/{MODEL}"), "displayName": "Fake Flash",
                 "inputTokenLimit": 32768, "outputTokenLimit": 2048,
                 "supportedGenerationMethods": ["generateContent", "countTokens"]},
                {"name": "models/fake-embedding", "supportedGenerationMethods": ["embedContent"]}
            ]})
            .to_string(),
        }
    } else if request.is_generation() {
        state
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Reply::status(500, r#"{"error":{"message":"no reply queued"}}"#))
    } else {
        Reply::status(404, r#"{"error":{"message":"unknown route"}}"#)
    };

    match reply {
        Reply::Sse(events) => {
            write_sse(&mut socket, &events).await;
        }
        Reply::SseThenHang(events) => {
            write_sse(&mut socket, &events).await;
            tokio::time::sleep(Duration::from_secs(120)).await;
        }
        Reply::Status {
            code,
            headers,
            body,
        } => {
            let mut head = format!(
                "HTTP/1.1 {code} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                body.len()
            );
            for (k, v) in headers {
                head.push_str(&format!("{k}: {v}\r\n"));
            }
            write(&mut socket, &format!("{head}\r\n{body}")).await;
        }
        Reply::Hang => tokio::time::sleep(Duration::from_secs(120)).await,
    }
    let _ = socket.shutdown().await;
}

async fn write_sse(socket: &mut TcpStream, events: &[String]) {
    write(
        socket,
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
    )
    .await;
    for event in events {
        write(socket, &format!("data: {event}\r\n\r\n")).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn write(socket: &mut TcpStream, text: &str) {
    let _ = socket.write_all(text.as_bytes()).await;
    let _ = socket.flush().await;
}

async fn read_request(socket: &mut TcpStream) -> Option<Received> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Received {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}
