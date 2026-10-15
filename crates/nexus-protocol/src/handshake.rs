use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::RequestId;

/// First message a client sends over the transport. Carries no secrets
/// (`SECURITATE_SI_VALIDARE_RO.md` §2.6) and no actor claim — the host derives
/// the actual actor from the authenticated connection, never from this payload.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct HandshakeRequest {
    pub request_id: RequestId,
    pub client_protocol_version: u32,
    pub client_name: String,
    /// Local pairing secret from `pairing::read_token` — proves the caller
    /// can read the host's per-profile token file, not a cryptographic
    /// identity. See `ARHITECTURA_NEXUS_RO.md` §6.
    pub pairing_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct HandshakeResponse {
    pub request_id: RequestId,
    pub server_protocol_version: u32,
    pub host_identity: String,
    pub compatible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
pub enum HealthStatus {
    Ok,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct GetHealthResponse {
    pub status: HealthStatus,
    /// u32, not u64: it crosses into JavaScript as a plain number, and specta
    /// refuses u64 by default to avoid silent precision loss above 2^53.
    pub uptime_seconds: u32,
}
