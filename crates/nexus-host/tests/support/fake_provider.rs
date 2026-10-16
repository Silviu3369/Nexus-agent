//! A scripted stand-in for any provider on 127.0.0.1, for tests only: a GET
//! is answered with the configured model list, each POST takes the next
//! queued reply. Every request is recorded as received.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

use super::fake_gemini::{Received, read_request, write, write_sse};

pub enum Canned {
    Json(u16, String),
    Sse(Vec<String>),
}

struct State {
    models: String,
    replies: Mutex<VecDeque<Canned>>,
    received: Mutex<Vec<Received>>,
}

pub struct FakeProvider {
    pub base_url: String,
    state: Arc<State>,
    accept: tokio::task::JoinHandle<()>,
}

impl FakeProvider {
    /// `models` is the JSON body answered to every GET.
    pub async fn start(models: serde_json::Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let state = Arc::new(State {
            models: models.to_string(),
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

    pub fn push(&self, reply: Canned) {
        self.state.replies.lock().unwrap().push_back(reply);
    }

    pub fn received(&self) -> Vec<Received> {
        self.state.received.lock().unwrap().clone()
    }
}

impl Drop for FakeProvider {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

async fn serve(mut socket: TcpStream, state: Arc<State>) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    let method = request.method.clone();
    state.received.lock().unwrap().push(request);
    let reply = if method == "GET" {
        Canned::Json(200, state.models.clone())
    } else {
        state
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| {
                Canned::Json(500, r#"{"error":{"message":"no reply queued"}}"#.into())
            })
    };
    match reply {
        Canned::Sse(events) => write_sse(&mut socket, &events).await,
        Canned::Json(code, body) => {
            write(
                &mut socket,
                &format!(
                    "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                ),
            )
            .await
        }
    }
    let _ = socket.shutdown().await;
}
