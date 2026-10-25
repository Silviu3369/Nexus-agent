//! The trusted tool registry (ADR-ACTION-001; `CONTRACTE_NEXUS_RO.md` §6):
//! every tool Nexus can run, with its effect class, how it may be retried,
//! the evidence its success has, how it can be undone, how it executes and
//! whether the user approves it first. Static and compiled in: a pack can
//! only choose among these tools and make a policy stricter, never add a
//! tool; the model's own claims about risk count for nothing.

use nexus_core::policy::{self, ToolPolicy};
use nexus_core::{EffectClass, Evidence, PolicyDecision};

use super::{document_file, markdown_artifact};

/// Contract §6: how a tool may be tried again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// No intended effect; a limited retry is allowed.
    ReadOnly,
    /// The destination deduplicates by a key.
    IdempotentWithKey,
    /// The resource is checked before deciding to go on.
    Reconcilable,
    /// Never repeated after an uncertain effect.
    NonRepeatable,
}

/// How an effect is undone, if at all (a new, authorized operation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compensation {
    None,
    /// The change is reversed in Nexus (a register change, a fact).
    Undo,
    /// The file is deleted, only if it is unchanged since it was written.
    DeleteIfUnchanged,
}

/// How a call is carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Execution {
    /// At once, within the reply's budgets (reads, register writes, and
    /// the user's own undo).
    Immediate,
    /// Recorded as a proposal the user decides elsewhere (a fact, in
    /// Memory); the reply goes on.
    Proposal,
    /// An effect run waiting for the user's approval of the exact manifest
    /// (intent, execution, verification, recovery: M1); the turn waits.
    EffectRun,
}

#[derive(Debug)]
pub struct ToolDescriptor {
    pub id: &'static str,
    /// The functions the model calls for it (MCP tools are matched by the
    /// server's offer instead).
    pub model_names: &'static [&'static str],
    pub version: u32,
    pub effect: EffectClass,
    pub retry: Retry,
    /// The proof its success has.
    pub evidence: Evidence,
    pub compensation: Compensation,
    pub execution: Execution,
    /// Approved by the user before it takes effect (a tool may be stricter
    /// than its class; see [`needs_approval`]).
    pub approval: bool,
    /// What it sends, and where.
    pub sends: &'static str,
}

pub const MCP_READ_ID: &str = "McpRead";
pub const MCP_CALL_ID: &str = "McpToolCall";
pub const RECORD_WRITE_ID: &str = "RecordWrite";
pub const MEMORY_FACT_ID: &str = "MemoryFact";
pub const UNDO_RECORD_ID: &str = "UndoRecordChange";
pub const RECORD_BATCH_ID: &str = "RecordWriteBatch";
pub const DELETE_DOCUMENT_ID: &str = "DeleteCreatedDocument";

pub static TOOLS: &[ToolDescriptor] = &[
    ToolDescriptor {
        id: "WebRead",
        model_names: &["fetch_url"],
        version: 1,
        effect: EffectClass::Read,
        retry: Retry::ReadOnly,
        evidence: Evidence::Reported,
        compensation: Compensation::None,
        execution: Execution::Immediate,
        approval: false,
        sends: "the address to the site (GET only); the page's text to the AI provider",
    },
    ToolDescriptor {
        id: "ListFiles",
        model_names: &["list_files"],
        version: 1,
        effect: EffectClass::Read,
        retry: Retry::ReadOnly,
        evidence: Evidence::Verified,
        compensation: Compensation::None,
        execution: Execution::Immediate,
        approval: false,
        sends: "the names of the files in a shared folder to the AI provider",
    },
    ToolDescriptor {
        id: "ReadDocument",
        model_names: &["read_document"],
        version: 1,
        effect: EffectClass::Read,
        retry: Retry::ReadOnly,
        evidence: Evidence::Verified,
        compensation: Compensation::None,
        execution: Execution::Immediate,
        approval: false,
        sends: "the document's text (up to its limit) to the AI provider",
    },
    ToolDescriptor {
        id: "FindRecords",
        model_names: &["find_records"],
        version: 1,
        effect: EffectClass::Read,
        retry: Retry::ReadOnly,
        evidence: Evidence::Verified,
        compensation: Compensation::None,
        execution: Execution::Immediate,
        approval: false,
        sends: "the records found to the AI provider",
    },
    ToolDescriptor {
        id: MCP_READ_ID,
        model_names: &[],
        version: 1,
        effect: EffectClass::Read,
        retry: Retry::ReadOnly,
        evidence: Evidence::Reported,
        compensation: Compensation::None,
        execution: Execution::Immediate,
        approval: false,
        sends: "the call's arguments to the MCP server; its answer to the AI provider",
    },
    ToolDescriptor {
        id: RECORD_WRITE_ID,
        model_names: &["add_record", "update_record"],
        version: 1,
        effect: EffectClass::InternalReversible,
        retry: Retry::Reconcilable,
        evidence: Evidence::Verified,
        compensation: Compensation::Undo,
        execution: Execution::Immediate,
        approval: false,
        sends: "nothing outside Nexus",
    },
    ToolDescriptor {
        id: MEMORY_FACT_ID,
        model_names: &["propose_memory"],
        version: 1,
        effect: EffectClass::InternalReversible,
        retry: Retry::Reconcilable,
        evidence: Evidence::Verified,
        compensation: Compensation::Undo,
        execution: Execution::Proposal,
        approval: true,
        sends: "nothing outside Nexus",
    },
    ToolDescriptor {
        id: markdown_artifact::TOOL_ID,
        model_names: &["propose_note"],
        version: markdown_artifact::TOOL_VERSION,
        effect: EffectClass::InternalReversible,
        retry: Retry::Reconcilable,
        evidence: Evidence::Verified,
        compensation: Compensation::None,
        execution: Execution::EffectRun,
        approval: true,
        sends: "nothing outside Nexus (a file in Nexus's own space)",
    },
    ToolDescriptor {
        id: document_file::TOOL_ID,
        model_names: &["propose_document"],
        version: document_file::TOOL_VERSION,
        effect: EffectClass::ExternalReversible,
        retry: Retry::Reconcilable,
        evidence: Evidence::Verified,
        compensation: Compensation::DeleteIfUnchanged,
        execution: Execution::EffectRun,
        approval: true,
        sends: "a new file in a folder the user shared for writing",
    },
    ToolDescriptor {
        id: MCP_CALL_ID,
        model_names: &[],
        version: 1,
        effect: EffectClass::ExternalIrreversible,
        retry: Retry::NonRepeatable,
        evidence: Evidence::Reported,
        compensation: Compensation::None,
        execution: Execution::EffectRun,
        approval: true,
        sends: "the call's arguments to the MCP server, which may change something there",
    },
    // The agent's register writes beyond what a reply makes without asking
    // (ADR-POLICY-001): the model still calls add_record and update_record;
    // the policy gathers those changes into one batch the user decides.
    ToolDescriptor {
        id: RECORD_BATCH_ID,
        model_names: &[],
        version: 1,
        effect: EffectClass::InternalReversible,
        retry: Retry::Reconcilable,
        evidence: Evidence::Verified,
        compensation: Compensation::Undo,
        execution: Execution::EffectRun,
        approval: true,
        sends: "nothing outside Nexus",
    },
    ToolDescriptor {
        id: UNDO_RECORD_ID,
        model_names: &[],
        version: 1,
        effect: EffectClass::InternalReversible,
        retry: Retry::Reconcilable,
        evidence: Evidence::Verified,
        compensation: Compensation::None,
        execution: Execution::Immediate,
        approval: false,
        sends: "nothing outside Nexus",
    },
    ToolDescriptor {
        id: DELETE_DOCUMENT_ID,
        model_names: &[],
        version: 1,
        effect: EffectClass::ExternalIrreversible,
        retry: Retry::Reconcilable,
        evidence: Evidence::Verified,
        compensation: Compensation::None,
        execution: Execution::Immediate,
        // The user's own undo request, confirmed, is its approval.
        approval: true,
        sends: "nothing; the file Nexus created is deleted",
    },
];

pub fn by_id(id: &str) -> Option<&'static ToolDescriptor> {
    TOOLS.iter().find(|t| t.id == id)
}

pub fn by_model_name(name: &str) -> Option<&'static ToolDescriptor> {
    TOOLS.iter().find(|t| t.model_names.contains(&name))
}

impl ToolDescriptor {
    /// What the approval policy knows about it (ADR-POLICY-001).
    pub fn policy(&self) -> ToolPolicy {
        ToolPolicy {
            class: self.effect,
            asks: self.approval,
        }
    }
}

/// What the approval policy decides for a call of `tool` (ADR-POLICY-001).
/// No rule exists yet, so it is the tool's default within the fixed limits.
pub fn decide(tool: &ToolDescriptor) -> PolicyDecision {
    policy::decide(tool.policy(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model can call it (the user's own undo is not offered).
    fn offered_to_model(tool: &ToolDescriptor) -> bool {
        !tool.model_names.is_empty() || tool.id == MCP_READ_ID || tool.id == MCP_CALL_ID
    }

    #[test]
    fn every_tool_is_registered_once_with_a_consistent_policy() {
        for (i, a) in TOOLS.iter().enumerate() {
            for b in &TOOLS[i + 1..] {
                assert_ne!(a.id, b.id);
                for name in a.model_names {
                    assert!(!b.model_names.contains(name), "{name} is claimed twice");
                }
            }
            match a.effect {
                EffectClass::Read => {
                    assert!(!a.approval, "{}", a.id);
                    assert_eq!(a.compensation, Compensation::None, "{}", a.id);
                    assert_eq!(a.execution, Execution::Immediate, "{}", a.id);
                    assert_eq!(a.retry, Retry::ReadOnly, "{}", a.id);
                }
                EffectClass::ExternalIrreversible | EffectClass::ExternalReversible => {
                    assert!(a.approval, "{}", a.id);
                    assert_eq!(decide(a), PolicyDecision::Ask, "{}", a.id);
                }
                EffectClass::InternalReversible => {}
            }
            // What the model calls runs as the policy decides: at once when
            // it needs no asking, otherwise as a proposal or an effect run.
            if offered_to_model(a) {
                let asks = a.execution != Execution::Immediate;
                assert_eq!(
                    decide(a),
                    if asks {
                        PolicyDecision::Ask
                    } else {
                        PolicyDecision::Auto
                    },
                    "{}",
                    a.id
                );
            }
            if a.effect == EffectClass::ExternalIrreversible {
                assert_eq!(a.compensation, Compensation::None, "{}", a.id);
            }
        }
    }

    #[test]
    fn every_function_offered_to_the_model_is_registered() {
        for name in [
            "fetch_url",
            "list_files",
            "read_document",
            "find_records",
            "add_record",
            "update_record",
            "propose_memory",
            "propose_note",
            "propose_document",
        ] {
            assert!(by_model_name(name).is_some(), "{name}");
        }
        assert!(by_model_name("run_shell").is_none());
        assert_eq!(
            by_model_name("propose_document").unwrap().effect,
            EffectClass::ExternalReversible
        );
        assert_eq!(
            by_id(MCP_CALL_ID).unwrap().effect,
            EffectClass::ExternalIrreversible
        );
    }
}
