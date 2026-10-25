//! DTOs for the user's approval policy (ADR-POLICY-001): every tool the agent
//! can use, what it does and sends, and what the user may change about it
//! within the fixed limits the Host enforces.

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::runs::ApprovalPath;

/// One tool and what the approval policy lets it do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct PermissionView {
    pub tool_id: String,
    /// What it lets the agent do, in plain words.
    pub label: String,
    /// Read, InternalReversible, ExternalReversible or ExternalIrreversible.
    pub effect_class: String,
    /// What it sends, and where.
    pub sends: String,
    /// Off: the agent is never offered it.
    pub enabled: bool,
    /// How it runs while on: Auto (without asking) or Ask (the user decides
    /// each time; an effect outside Nexus in a Windows dialog).
    pub decision: String,
    /// For register writes: how many a reply makes without asking (the
    /// rest wait in one batch), and the default.
    pub max_auto: Option<u32>,
    pub default_max_auto: Option<u32>,
    /// When the user last changed it (`None`: the default holds).
    pub changed_at_ms: Option<i64>,
}

/// A change to one tool. Turning a tool back on or letting more register
/// writes run without asking loosens the policy: the Host accepts it only
/// from the native Windows dialog the app shows itself.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct SetPermissionRequest {
    pub tool_id: String,
    pub enabled: bool,
    /// For register writes only.
    pub max_auto: Option<u32>,
    #[serde(default)]
    pub via: ApprovalPath,
}
