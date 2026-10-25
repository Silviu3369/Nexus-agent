//! DTOs for what data may leave for which AI provider (ADR-PROVIDER-002):
//! what each connection may receive and why, each source's class, and the
//! changes the user can make. Letting more data leave is accepted by the
//! Host only from the native Windows dialog.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::ids::ConnectionId;
use crate::runs::ApprovalPath;

/// What leaves Nexus, and for which connection.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DataPolicyView {
    /// The user says they use Nexus from the EEA, Switzerland or the UK.
    pub in_eea: bool,
    /// When Nexus last checked the providers' terms (YYYY-MM-DD).
    pub terms_checked: String,
    pub connections: Vec<ConnectionDataView>,
    pub sources: Vec<SourceDataView>,
}

/// What one connection may receive, and why.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ConnectionDataView {
    pub connection_id: ConnectionId,
    pub display_name: String,
    pub provider: String,
    pub free_tier: bool,
    /// Public, Personal or Business.
    pub can_receive: String,
    /// Why, in plain words.
    pub reason: String,
    /// The provider's terms Nexus read; empty when not confirmed.
    pub terms_url: String,
    /// Included, Offered (the user signs it), None or Unknown.
    pub dpa: String,
    pub dpa_signed: bool,
    pub allows_training: bool,
}

/// A source the agent may read and send: a shared folder, a register, an
/// MCP server.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SourceDataView {
    /// Folder, Register or McpServer.
    pub kind: String,
    pub id: String,
    pub name: String,
    /// Personal or Business.
    pub data_class: String,
}

/// One change to what data may leave.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub enum DataChange {
    /// Where the user uses Nexus from.
    InEea { in_eea: bool },
    /// What the user says about one connection.
    Connection {
        connection_id: ConnectionId,
        dpa_signed: bool,
        allows_training: bool,
    },
    /// A source's class.
    Source {
        kind: String,
        id: String,
        data_class: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SetDataPolicyRequest {
    pub change: DataChange,
    #[serde(default)]
    pub via: ApprovalPath,
}
