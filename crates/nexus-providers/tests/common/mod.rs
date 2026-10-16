//! A controlled local HTTP server shared by the provider protocol tests:
//! it serves scripted replies and records exactly what each request held.

#![allow(dead_code)]

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Default)]
pub struct Captured {
    pub request_line: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Captured {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn body_json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).expect("the adapter sends JSON")
    }
}

pub enum Reply {
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
pub async fn server(replies: Vec<Reply>) -> (String, tokio::task::JoinHandle<Vec<Captured>>) {
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

pub async fn write(socket: &mut tokio::net::TcpStream, text: &str) {
    let _ = socket.write_all(text.as_bytes()).await;
    let _ = socket.flush().await;
}

pub async fn read_request(socket: &mut tokio::net::TcpStream) -> Captured {
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
