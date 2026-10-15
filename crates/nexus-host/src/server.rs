use std::io;
use std::time::Instant;

use futures::StreamExt;
use nexus_protocol::service::NexusService;
use nexus_protocol::{
    ErrorCode, GetHealthResponse, HandshakeRequest, HandshakeResponse, HealthStatus, NexusError,
    PROTOCOL_VERSION,
};
use tarpc::context::Context;
use tarpc::server::{BaseChannel, Channel};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio_serde::formats::Json;
use tokio_util::codec::LengthDelimitedCodec;

/// IPC-02: reject a message above this size before it is ever deserialized.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// OS error raised by `CreateNamedPipe` when `first_pipe_instance(true)` and
/// an instance already exists — our LIFE-02 single-instance signal.
const ERROR_PIPE_BUSY: i32 = 231;

#[derive(Clone)]
pub struct HostServer {
    pub pairing_token: String,
    pub started_at: Instant,
}

impl NexusService for HostServer {
    async fn handshake(
        self,
        _ctx: Context,
        request: HandshakeRequest,
    ) -> Result<HandshakeResponse, NexusError> {
        if request.pairing_token != self.pairing_token {
            tracing::warn!("rejected handshake with invalid pairing token");
            return Err(NexusError::new(
                ErrorCode::Unauthenticated,
                "pairing token invalid",
            ));
        }
        if request.client_protocol_version != PROTOCOL_VERSION {
            tracing::warn!(
                client_version = request.client_protocol_version,
                host_version = PROTOCOL_VERSION,
                "rejected handshake with incompatible protocol version"
            );
            return Err(NexusError::new(
                ErrorCode::UnsupportedVersion,
                format!(
                    "client protocol {} incompatible with host protocol {}",
                    request.client_protocol_version, PROTOCOL_VERSION
                ),
            ));
        }
        tracing::info!(client = %request.client_name, "handshake accepted");
        Ok(HandshakeResponse {
            request_id: request.request_id,
            server_protocol_version: PROTOCOL_VERSION,
            host_identity: "nexus-host".to_string(),
            compatible: true,
        })
    }

    async fn get_health(self, _ctx: Context) -> Result<GetHealthResponse, NexusError> {
        Ok(GetHealthResponse {
            status: HealthStatus::Ok,
            uptime_seconds: self.started_at.elapsed().as_secs(),
        })
    }
}

/// Binds the first instance of the named pipe. Fails if another host already
/// owns this pipe name (LIFE-02) instead of silently becoming a second queue
/// owner.
pub fn bind_first_instance(pipe_name: &str) -> anyhow::Result<NamedPipeServer> {
    match ServerOptions::new()
        .first_pipe_instance(true)
        .create(pipe_name)
    {
        Ok(pipe) => Ok(pipe),
        Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
            anyhow::bail!(
                "another nexus-host instance is already running for this profile (pipe {pipe_name} busy)"
            );
        }
        Err(err) => Err(err.into()),
    }
}

/// Runs the accept loop forever. `first` must come from `bind_first_instance`
/// so the single-instance guarantee holds for the life of the process.
pub async fn serve_forever(
    mut listener: NamedPipeServer,
    pipe_name: String,
    pairing_token: String,
) -> anyhow::Result<()> {
    let started_at = Instant::now();

    loop {
        listener.connect().await?;
        let connected = listener;
        // Open the next instance immediately so a second client can queue
        // while this one is served.
        listener = ServerOptions::new().create(&pipe_name)?;

        let server = HostServer {
            pairing_token: pairing_token.clone(),
            started_at,
        };

        tokio::spawn(async move {
            if let Err(err) = serve_connection(connected, server).await {
                tracing::warn!(error = %err, "connection ended with error");
            }
        });
    }
}

async fn serve_connection(pipe: NamedPipeServer, server: HostServer) -> io::Result<()> {
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME_BYTES)
        .new_codec();
    let framed = tokio_util::codec::Framed::new(pipe, codec);
    let transport = tarpc::serde_transport::new(framed, Json::default());
    let channel = BaseChannel::with_defaults(transport);

    channel
        .execute(server.serve())
        .for_each(|response_future| async move {
            tokio::spawn(response_future);
        })
        .await;

    Ok(())
}
