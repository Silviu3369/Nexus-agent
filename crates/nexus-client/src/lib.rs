//! IPC client reused by the Tauri shell (`ARHITECTURA_NEXUS_RO.md` §5).
//! Connects to `nexus-host` over the same named pipe + framed-JSON tarpc
//! transport the host serves, and presents the local pairing token.

use nexus_protocol::service::NexusServiceClient;
use nexus_protocol::{HandshakeRequest, HandshakeResponse, NexusError, PROTOCOL_VERSION, pairing};
use tarpc::context;
use tokio::net::windows::named_pipe::ClientOptions;
use tokio_serde::formats::Json;
use tokio_util::codec::LengthDelimitedCodec;

const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("could not read local pairing token: {0}")]
    MissingPairingToken(#[source] std::io::Error),
    #[error("could not connect to nexus-host pipe: {0}")]
    Pipe(#[source] std::io::Error),
    #[error("host rejected handshake: {0}")]
    HandshakeRejected(#[source] NexusError),
}

pub struct NexusClient {
    client: NexusServiceClient,
}

impl NexusClient {
    /// Connects to the host's named pipe for the current Windows account and
    /// performs the handshake. Returns the connected client plus the host's
    /// handshake response (so callers can check `compatible` before use).
    pub async fn connect(
        client_name: impl Into<String>,
    ) -> Result<(Self, HandshakeResponse), ConnectError> {
        let pairing_token = pairing::read_token().map_err(ConnectError::MissingPairingToken)?;
        Self::connect_with(&pairing::pipe_name(), pairing_token, client_name).await
    }

    /// Lower-level entry point used by tests and by anything that must talk
    /// to a host on a non-default pipe name (e.g. an isolated test host).
    pub async fn connect_with(
        pipe_name: &str,
        pairing_token: String,
        client_name: impl Into<String>,
    ) -> Result<(Self, HandshakeResponse), ConnectError> {
        let pipe = ClientOptions::new()
            .open(pipe_name)
            .map_err(ConnectError::Pipe)?;

        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(MAX_FRAME_BYTES)
            .new_codec();
        let framed = tokio_util::codec::Framed::new(pipe, codec);
        let transport = tarpc::serde_transport::new(framed, Json::default());

        let client = NexusServiceClient::new(tarpc::client::Config::default(), transport).spawn();

        let request = HandshakeRequest {
            request_id: nexus_protocol::RequestId::new(),
            client_protocol_version: PROTOCOL_VERSION,
            client_name: client_name.into(),
            pairing_token,
        };

        let response = client
            .handshake(context::current(), request)
            .await
            .map_err(|e| ConnectError::Pipe(std::io::Error::other(e)))?
            .map_err(ConnectError::HandshakeRejected)?;

        Ok((Self { client }, response))
    }

    pub async fn get_health(
        &self,
    ) -> Result<Result<nexus_protocol::GetHealthResponse, NexusError>, tarpc::client::RpcError>
    {
        self.client.get_health(context::current()).await
    }
}
