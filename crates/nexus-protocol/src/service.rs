//! The RPC contract itself, generated from a single Rust trait
//! (ADR-IPC-001: tarpc instead of a separate schema language). `nexus-host`
//! implements this; `nexus-client` calls the generated client stub.

use crate::error::NexusError;
use crate::handshake::{GetHealthResponse, HandshakeRequest, HandshakeResponse};

#[tarpc::service]
pub trait NexusService {
    /// First call on a new connection. Negotiates protocol version and
    /// identity; carries no secrets and no actor claim.
    async fn handshake(request: HandshakeRequest) -> Result<HandshakeResponse, NexusError>;

    /// Liveness/identity probe used after handshake (`CONTRACTE_NEXUS_RO.md` §8).
    async fn get_health() -> Result<GetHealthResponse, NexusError>;
}
