//! Real named-pipe + tarpc integration tests for IPC-01 / IPC-02 / LIFE-02
//! (`SECURITATE_SI_VALIDARE_RO.md` §3). No mocked transport: this binds an
//! actual Windows named pipe and connects real clients to it.

use nexus_client::{ConnectError, NexusClient};
use nexus_protocol::service::NexusServiceClient;
use nexus_protocol::{ErrorCode, HandshakeRequest, PROTOCOL_VERSION, RequestId};

fn unique_pipe_name() -> String {
    format!(r"\\.\pipe\nexus-test-{}", uuid::Uuid::new_v4())
}

async fn start_test_host(pipe_name: String, pairing_token: String) {
    let listener = nexus_host::bind_first_instance(&pipe_name)
        .expect("binding a fresh, uniquely-named pipe must succeed");
    tokio::spawn(nexus_host::serve_forever(
        listener,
        pipe_name,
        pairing_token,
    ));
    // Give the accept loop a tick to start listening before clients connect.
    tokio::task::yield_now().await;
}

/// A client that skips `NexusClient`'s guarantees (real version, size cap,
/// handshake first), so tests can prove the *host* enforces each rule.
fn raw_client(pipe_name: &str, max_frame_bytes: usize) -> NexusServiceClient {
    let pipe = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(pipe_name)
        .expect("pipe must accept a connection");
    let codec = tokio_util::codec::LengthDelimitedCodec::builder()
        .max_frame_length(max_frame_bytes)
        .new_codec();
    let framed = tokio_util::codec::Framed::new(pipe, codec);
    let transport = tarpc::serde_transport::new(framed, tokio_serde::formats::Json::default());
    NexusServiceClient::new(tarpc::client::Config::default(), transport).spawn()
}

/// Opens a bare client pipe, waiting out the instant between two accepts when
/// the host has not yet created its next free instance.
async fn open_raw_pipe(pipe_name: &str) -> tokio::net::windows::named_pipe::NamedPipeClient {
    const ERROR_PIPE_BUSY: i32 = 231;
    for _ in 0..50 {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(pipe_name) {
            Ok(pipe) => return pipe,
            Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(err) => panic!("pipe must accept a connection: {err}"),
        }
    }
    panic!("pipe stayed busy for 1 s");
}

fn handshake_request(pairing_token: &str, protocol_version: u32) -> HandshakeRequest {
    HandshakeRequest {
        request_id: RequestId::new(),
        client_protocol_version: protocol_version,
        client_name: "raw-test-client".to_string(),
        pairing_token: pairing_token.to_string(),
    }
}

#[tokio::test]
async fn ipc01_legitimate_client_handshakes_successfully() {
    let pipe_name = unique_pipe_name();
    let token = "correct-token".to_string();
    start_test_host(pipe_name.clone(), token.clone()).await;

    let (client, response) = NexusClient::connect_with(&pipe_name, token, "test-client")
        .await
        .expect("legitimate client with correct token must connect");

    assert!(response.compatible);
    assert_eq!(response.server_protocol_version, PROTOCOL_VERSION);

    let health = client
        .get_health()
        .await
        .expect("rpc transport must succeed")
        .expect("get_health must succeed after a valid handshake");
    assert_eq!(health.status, nexus_protocol::HealthStatus::Ok);
}

#[tokio::test]
async fn ipc02_wrong_pairing_token_is_rejected() {
    let pipe_name = unique_pipe_name();
    start_test_host(pipe_name.clone(), "real-token".to_string()).await;

    let result = NexusClient::connect_with(&pipe_name, "wrong-token".to_string(), "attacker").await;

    match result {
        Ok(_) => panic!("expected handshake rejection for a bad token, got success"),
        Err(ConnectError::HandshakeRejected(err)) => {
            assert_eq!(err.code, ErrorCode::Unauthenticated);
        }
        Err(other) => panic!("expected HandshakeRejected, got {other:?}"),
    }
}

#[tokio::test]
async fn ipc02_incompatible_protocol_version_is_rejected() {
    let pipe_name = unique_pipe_name();
    let token = "correct-token";
    start_test_host(pipe_name.clone(), token.to_string()).await;

    let client = raw_client(&pipe_name, nexus_host::server::MAX_FRAME_BYTES);
    let response = client
        .handshake(
            tarpc::context::current(),
            handshake_request(token, PROTOCOL_VERSION + 999),
        )
        .await
        .expect("rpc transport must succeed");

    match response {
        Err(err) => assert_eq!(err.code, ErrorCode::UnsupportedVersion),
        Ok(_) => panic!("expected the host to reject an incompatible protocol version"),
    }
}

#[tokio::test]
async fn ipc02_calls_before_handshake_are_rejected() {
    let pipe_name = unique_pipe_name();
    start_test_host(pipe_name.clone(), "correct-token".to_string()).await;

    let client = raw_client(&pipe_name, nexus_host::server::MAX_FRAME_BYTES);
    let response = client
        .get_health(tarpc::context::current())
        .await
        .expect("rpc transport must succeed");

    match response {
        Err(err) => assert_eq!(err.code, ErrorCode::Unauthenticated),
        Ok(_) => panic!("the host must not serve calls on a connection that never handshook"),
    }
}

#[tokio::test]
async fn ipc02_failed_handshake_does_not_open_a_session() {
    let pipe_name = unique_pipe_name();
    let token = "correct-token";
    start_test_host(pipe_name.clone(), token.to_string()).await;

    let client = raw_client(&pipe_name, nexus_host::server::MAX_FRAME_BYTES);

    // A valid handshake opens the session...
    client
        .handshake(
            tarpc::context::current(),
            handshake_request(token, PROTOCOL_VERSION),
        )
        .await
        .expect("rpc transport must succeed")
        .expect("valid handshake must succeed");

    // ...and a later bad one on the same connection must close it again.
    let rejected = client
        .handshake(
            tarpc::context::current(),
            handshake_request("wrong-token", PROTOCOL_VERSION),
        )
        .await
        .expect("rpc transport must succeed");
    assert!(rejected.is_err());

    let response = client
        .get_health(tarpc::context::current())
        .await
        .expect("rpc transport must succeed");
    match response {
        Err(err) => assert_eq!(err.code, ErrorCode::Unauthenticated),
        Ok(_) => panic!("a failed handshake must revoke the connection's session"),
    }
}

#[tokio::test]
async fn ipc02_oversized_message_is_rejected() {
    let pipe_name = unique_pipe_name();
    let token = "correct-token";
    start_test_host(pipe_name.clone(), token.to_string()).await;

    // This client's own cap is far above the host's 1 MiB, so it will send the
    // oversized frame and the test shows the host is the one refusing it.
    let client = raw_client(&pipe_name, 16 * 1024 * 1024);
    let mut request = handshake_request(token, PROTOCOL_VERSION);
    // Comfortably over the host's MAX_FRAME_BYTES once JSON-encoded.
    request.client_name = "a".repeat(2 * 1024 * 1024);

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.handshake(tarpc::context::current(), request),
    )
    .await;

    match outcome {
        // Timed out (host dropped the connection) or transport error: refused.
        Err(_) | Ok(Err(_)) => {}
        Ok(Ok(Ok(_))) => panic!("host must not accept a handshake carried in an oversized frame"),
        // Decoded but still refused the call: acceptable too.
        Ok(Ok(Err(_))) => {}
    }
}

#[tokio::test]
async fn ipc02_pipe_server_running_as_another_user_is_not_trusted() {
    let pipe_name = unique_pipe_name();
    start_test_host(pipe_name.clone(), "correct-token".to_string()).await;

    // The real server process (this test's own) runs as the current user.
    let me = nexus_client::current_user_sid().expect("current SID must be readable");
    nexus_client::verify_pipe_server_user(&open_raw_pipe(&pipe_name).await, &me)
        .expect("a server running as the current user must be trusted");

    // Expecting any other account (here: LocalSystem) must refuse the same
    // real server — this is the check that stops a squatter on our pipe name
    // before the pairing token is ever sent.
    match nexus_client::verify_pipe_server_user(&open_raw_pipe(&pipe_name).await, "S-1-5-18") {
        Err(ConnectError::UntrustedServer { expected, actual }) => {
            assert_eq!(expected, "S-1-5-18");
            assert_eq!(actual, me);
        }
        other => panic!("expected UntrustedServer, got {other:?}"),
    }
}

#[tokio::test]
async fn life02_second_instance_on_same_pipe_name_is_rejected() {
    let pipe_name = unique_pipe_name();
    let _first = nexus_host::bind_first_instance(&pipe_name)
        .expect("first instance must bind the fresh pipe name");

    let second = nexus_host::bind_first_instance(&pipe_name);
    let err = second.expect_err("a second instance must not be able to bind the same pipe name");
    assert!(
        err.to_string().contains("already owned"),
        "the refusal must be recognised as an ownership conflict, got: {err}"
    );
}
