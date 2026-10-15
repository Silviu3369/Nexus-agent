//! Real named-pipe + tarpc integration tests for IPC-01 / IPC-02
//! (`SECURITATE_SI_VALIDARE_RO.md` §3). No mocked transport: this binds an
//! actual Windows named pipe and connects a real `nexus-client` to it.

use nexus_client::{ConnectError, NexusClient};
use nexus_protocol::PROTOCOL_VERSION;

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
            assert_eq!(err.code, nexus_protocol::ErrorCode::Unauthenticated);
        }
        Err(other) => panic!("expected HandshakeRejected, got {other:?}"),
    }
}

#[tokio::test]
async fn ipc02_incompatible_protocol_version_is_rejected() {
    let pipe_name = unique_pipe_name();
    let token = "correct-token".to_string();
    start_test_host(pipe_name.clone(), token.clone()).await;

    // Build the request by hand with a bogus protocol version instead of
    // going through `NexusClient::connect_with`, which always sends the
    // real `PROTOCOL_VERSION`.
    let pipe = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(&pipe_name)
        .expect("pipe must accept a connection");
    let codec = tokio_util::codec::LengthDelimitedCodec::builder()
        .max_frame_length(nexus_host::server::MAX_FRAME_BYTES)
        .new_codec();
    let framed = tokio_util::codec::Framed::new(pipe, codec);
    let transport = tarpc::serde_transport::new(framed, tokio_serde::formats::Json::default());
    let client = nexus_protocol::service::NexusServiceClient::new(
        tarpc::client::Config::default(),
        transport,
    )
    .spawn();

    let request = nexus_protocol::HandshakeRequest {
        request_id: nexus_protocol::RequestId::new(),
        client_protocol_version: PROTOCOL_VERSION + 999,
        client_name: "future-client".to_string(),
        pairing_token: token,
    };

    let response = client
        .handshake(tarpc::context::current(), request)
        .await
        .expect("rpc transport must succeed");

    match response {
        Err(err) => assert_eq!(err.code, nexus_protocol::ErrorCode::UnsupportedVersion),
        Ok(_) => panic!("expected the host to reject an incompatible protocol version"),
    }
}

#[tokio::test]
async fn ipc02_oversized_message_is_rejected() {
    let pipe_name = unique_pipe_name();
    let token = "correct-token".to_string();
    start_test_host(pipe_name.clone(), token.clone()).await;

    // This client deliberately does NOT respect the host's 1 MiB cap, so the
    // test proves the *host* enforces it, not that a well-behaved client
    // chose to.
    let pipe = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(&pipe_name)
        .expect("pipe must accept a connection");
    let oversized_codec = tokio_util::codec::LengthDelimitedCodec::builder()
        .max_frame_length(16 * 1024 * 1024)
        .new_codec();
    let framed = tokio_util::codec::Framed::new(pipe, oversized_codec);
    let transport = tarpc::serde_transport::new(framed, tokio_serde::formats::Json::default());
    let client = nexus_protocol::service::NexusServiceClient::new(
        tarpc::client::Config::default(),
        transport,
    )
    .spawn();

    let request = nexus_protocol::HandshakeRequest {
        request_id: nexus_protocol::RequestId::new(),
        client_protocol_version: PROTOCOL_VERSION,
        // Comfortably over the host's MAX_FRAME_BYTES once JSON-encoded.
        client_name: "a".repeat(2 * 1024 * 1024),
        pairing_token: token,
    };

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.handshake(tarpc::context::current(), request),
    )
    .await;

    match outcome {
        Err(_) => { /* timed out waiting for a response: the host closed/never answered — rejected */
        }
        Ok(Err(_rpc_error)) => { /* transport error: the host refused the oversized frame */ }
        Ok(Ok(Ok(_))) => panic!("host must not accept a handshake carried in an oversized frame"),
        Ok(Ok(Err(_app_error))) => { /* host decoded it but still refused the call: acceptable too */
        }
    }
}

#[tokio::test]
async fn life02_second_instance_on_same_pipe_name_is_rejected() {
    let pipe_name = unique_pipe_name();
    let _first = nexus_host::bind_first_instance(&pipe_name)
        .expect("first instance must bind the fresh pipe name");

    let second = nexus_host::bind_first_instance(&pipe_name);
    assert!(
        second.is_err(),
        "a second instance must not be able to bind the same pipe name as first instance"
    );
}
