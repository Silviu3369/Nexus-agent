//! Closed state machines for Run and Action (`CONTRACTE_NEXUS_RO.md` §4).
//! Transitions are validated here, server-side; storage refuses any state
//! string these enums do not produce.

use std::fmt;
use std::str::FromStr;

macro_rules! closed_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => stringify!($variant)),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnknownState;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $(stringify!($variant) => Ok($name::$variant),)+
                    other => Err(UnknownState(other.to_string())),
                }
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown state {0:?}")]
pub struct UnknownState(pub String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal transition {from} -> {to}")]
pub struct IllegalTransition {
    pub from: &'static str,
    pub to: &'static str,
}

closed_enum!(RunState {
    Queued,
    Running,
    AwaitingApproval,
    Verifying,
    CancellationRequested,
    NeedsReconciliation,
    NeedsReview,
    Succeeded,
    Failed,
    Cancelled,
    Denied,
});

impl RunState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunState::Succeeded | RunState::Failed | RunState::Cancelled | RunState::Denied
        )
    }

    pub fn can_transition_to(self, next: RunState) -> bool {
        use RunState::*;
        match self {
            Queued => matches!(next, Running | Cancelled),
            Running => matches!(
                next,
                AwaitingApproval | Verifying | Failed | CancellationRequested | NeedsReconciliation
            ),
            // After approval the run is queued again; it never resumes a
            // sleeping worker on the strength of an old decision.
            AwaitingApproval => matches!(next, Queued | Denied | Failed | Cancelled),
            Verifying => matches!(next, Succeeded | Failed | NeedsReconciliation),
            // Effects already in flight may still land, so a cancellation can
            // end in any outcome that reports them truthfully.
            CancellationRequested => matches!(
                next,
                Cancelled | Verifying | Succeeded | Failed | NeedsReconciliation
            ),
            // Cancelled: reconciliation proved no effect for a run whose
            // cancellation had been requested before it was interrupted.
            NeedsReconciliation => {
                matches!(next, Queued | Succeeded | Failed | NeedsReview | Cancelled)
            }
            NeedsReview => matches!(next, Succeeded | Failed | Cancelled),
            Succeeded | Failed | Cancelled | Denied => false,
        }
    }

    pub fn transition(self, next: RunState) -> Result<RunState, IllegalTransition> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(IllegalTransition {
                from: self.as_str(),
                to: next.as_str(),
            })
        }
    }
}

closed_enum!(ActionState {
    Prepared,
    AwaitingApproval,
    Authorized,
    Executing,
    Verified,
    Failed,
    NeedsReconciliation,
    Denied,
    Expired,
    Invalidated,
});

impl ActionState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            ActionState::Verified
                | ActionState::Failed
                | ActionState::Denied
                | ActionState::Expired
                | ActionState::Invalidated
        )
    }

    pub fn can_transition_to(self, next: ActionState) -> bool {
        use ActionState::*;
        match self {
            Prepared => matches!(next, AwaitingApproval | Authorized | Invalidated),
            // Refusal, expiry and invalidation are explicit outcomes, not
            // errors that trigger a retry.
            AwaitingApproval => matches!(next, Authorized | Denied | Expired | Invalidated),
            Authorized => matches!(next, Executing | Invalidated),
            Executing => matches!(next, Verified | Failed | NeedsReconciliation),
            NeedsReconciliation => matches!(next, Verified | Failed | Executing),
            Verified | Failed | Denied | Expired | Invalidated => false,
        }
    }

    pub fn transition(self, next: ActionState) -> Result<ActionState, IllegalTransition> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(IllegalTransition {
                from: self.as_str(),
                to: next.as_str(),
            })
        }
    }
}

closed_enum!(OperationState {
    Intended,
    Verified,
    Failed,
    NeedsReview,
});

impl OperationState {
    /// `Intended` is written before any side effect. A host that restarts and
    /// finds one decides the outcome by inspecting the resource — never by
    /// replaying the effect blindly (`CONTRACTE_NEXUS_RO.md` §5, §7).
    pub fn can_transition_to(self, next: OperationState) -> bool {
        use OperationState::*;
        matches!(
            (self, next),
            (Intended, Verified | Failed | NeedsReview) | (NeedsReview, Verified | Failed)
        )
    }
}

closed_enum!(MessageRole { User, Model });

// The user's speed/quality choice for models that support it (Gemini 3.x;
// `nexus_providers::ThinkingLevel` is the matching provider-level type).
// Not a workflow state — stored on the connection, not transitioned.
closed_enum!(ThinkingLevel { Low, Medium, High });

// What the agent did while producing a reply, shown to the user as it
// happens. Visibility, not authority: a step grants nothing.
closed_enum!(AgentStepKind {
    ModelCall,
    Fetch,
    NoteProposed,
    MemoryProposed,
    McpCall,
    RecordsFound,
    RecordChanged,
    FilesListed,
    DocumentRead,
    DocumentProposed,
});

closed_enum!(AgentStepStatus {
    Running,
    Done,
    Failed,
});

impl AgentStepStatus {
    /// Written as Running when the step starts, settled exactly once.
    pub fn can_transition_to(self, next: AgentStepStatus) -> bool {
        use AgentStepStatus::*;
        matches!((self, next), (Running, Done | Failed))
    }
}

// Whose memory a fact is: about the user across every project, or about
// one project only.
closed_enum!(MemoryScope { Personal, Project });

// A fact the agent proposes waits for the user; only Active facts are ever
// sent to a model. The user's own facts start Active.
closed_enum!(MemoryStatus {
    Proposed,
    Active,
    Rejected,
});

closed_enum!(MemorySource { User, Agent });

// Who changed a record in one of Nexus's registers (`PLAN_NEXUS_RO.md` §3a):
// the agent writes directly, and the user can undo each of its changes.
closed_enum!(RecordActor { User, Agent });

// One entry of a record's history. An undo is itself an entry; the entry it
// reverses is marked undone.
closed_enum!(RecordChangeKind {
    Created,
    Updated,
    Undo,
});

impl MemoryStatus {
    /// A proposal is decided once; a decided fact is edited or deleted,
    /// never re-proposed.
    pub fn can_transition_to(self, next: MemoryStatus) -> bool {
        use MemoryStatus::*;
        matches!((self, next), (Proposed, Active | Rejected))
    }
}

closed_enum!(MessageStatus {
    Streaming,
    Complete,
    Failed,
    Cancelled,
});

impl MessageStatus {
    /// A model message is written once as Streaming and settles exactly once.
    pub fn can_transition_to(self, next: MessageStatus) -> bool {
        use MessageStatus::*;
        matches!((self, next), (Streaming, Complete | Failed | Cancelled))
    }
}

// What started a task (ADR-TASK-001): the user in a conversation, an
// automation, an item taken in by an intake source, a note the user sent
// directly (M1), or the agent itself (an effect it proposed during a turn).
closed_enum!(TaskOrigin {
    Direct,
    User,
    Automation,
    Intake,
    Agent,
});

// Who does or asks for something (ADR-IDENTITY-001): the user, the agent,
// an automation, an intake source (the inbox), a channel (M4), or the Host
// itself (recovery, expiry). Only a `User` decides.
closed_enum!(ActorKind {
    User,
    Agent,
    Automation,
    Intake,
    Channel,
    System,
});

// A run is either an effect with its own action (M1: a note, a document, an
// MCP call) or one turn of the agent: a request and every reply to it.
closed_enum!(RunType { Effect, Agent });

// What the user did about the agent's work (ADR-TASK-001): decided a
// proposal, undid or corrected a change, tried again, or stopped it.
closed_enum!(InterventionKind {
    Approved,
    Rejected,
    Undo,
    Correction,
    Retry,
    Stopped,
});

impl InterventionKind {
    /// Counts against accepting the turn it concerns: the user turned down,
    /// undid, corrected or stopped what the agent did.
    pub fn is_negative(self) -> bool {
        use InterventionKind::*;
        matches!(self, Rejected | Undo | Correction | Stopped)
    }
}

// What a tool's effect is (ADR-ACTION-001): it only reads; it changes
// Nexus's own state and can be undone; it changes something outside Nexus
// that can be put back; or something that cannot.
closed_enum!(EffectClass {
    Read,
    InternalReversible,
    ExternalReversible,
    ExternalIrreversible,
});

// What the approval policy decides for a call (ADR-POLICY-001): carry it
// out, ask the user first, or refuse it.
closed_enum!(PolicyDecision { Auto, Ask, Deny });

// How well an action's success is proven (ADR-ACTION-001): read back and
// checked, only reported by the service, or not checkable at all.
closed_enum!(Evidence {
    Verified,
    Reported,
    Unverifiable,
});

// Why a run failed (ADR-TASK-001), a closed list. `Internal` covers what no
// other cause names.
closed_enum!(FailureCause {
    ProviderUnavailable,
    QuotaExceeded,
    BudgetExceeded,
    InvalidModelOutput,
    ValidationRefused,
    VerificationMismatch,
    PermissionDenied,
    UserRejected,
    Timeout,
    HostInterrupted,
    OutcomeUnknown,
    Internal,
});

impl FailureCause {
    /// The cause of a reply that settled with `code` (an error code name,
    /// or `Interrupted` when the Host stopped during the reply). Kept in
    /// step with the same mapping in schema version 15.
    pub fn from_error_code(code: &str) -> FailureCause {
        use FailureCause::*;
        match code {
            "QuotaExceeded" => QuotaExceeded,
            "ProviderUnavailable" | "Busy" => ProviderUnavailable,
            "DeadlineExceeded" => Timeout,
            "BudgetExceeded" => BudgetExceeded,
            "Unauthenticated" | "Forbidden" => PermissionDenied,
            "InvalidRequest" => ValidationRefused,
            "Interrupted" => HostInterrupted,
            "OutcomeUnknown" => OutcomeUnknown,
            _ => Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_names_round_trip() {
        for s in RunState::ALL {
            assert_eq!(s.as_str().parse::<RunState>().unwrap(), *s);
        }
        for s in ActionState::ALL {
            assert_eq!(s.as_str().parse::<ActionState>().unwrap(), *s);
        }
        for s in OperationState::ALL {
            assert_eq!(s.as_str().parse::<OperationState>().unwrap(), *s);
        }
        assert!("Approved=true".parse::<RunState>().is_err());
    }

    #[test]
    fn a_failed_reply_names_one_cause_from_a_closed_list() {
        assert_eq!(
            FailureCause::from_error_code("QuotaExceeded"),
            FailureCause::QuotaExceeded
        );
        assert_eq!(
            FailureCause::from_error_code("Busy"),
            FailureCause::ProviderUnavailable
        );
        assert_eq!(
            FailureCause::from_error_code("DeadlineExceeded"),
            FailureCause::Timeout
        );
        assert_eq!(
            FailureCause::from_error_code("Interrupted"),
            FailureCause::HostInterrupted
        );
        assert_eq!(
            FailureCause::from_error_code("Forbidden"),
            FailureCause::PermissionDenied
        );
        assert_eq!(
            FailureCause::from_error_code("anything else"),
            FailureCause::Internal
        );
        for c in FailureCause::ALL {
            assert_eq!(c.as_str().parse::<FailureCause>().unwrap(), *c);
        }
    }

    #[test]
    fn terminal_states_have_no_exits() {
        for from in RunState::ALL.iter().filter(|s| s.is_terminal()) {
            for to in RunState::ALL {
                assert!(!from.can_transition_to(*to), "{from} -> {to}");
            }
        }
        for from in ActionState::ALL.iter().filter(|s| s.is_terminal()) {
            for to in ActionState::ALL {
                assert!(!from.can_transition_to(*to), "{from} -> {to}");
            }
        }
    }

    #[test]
    fn documented_happy_path_is_allowed() {
        use RunState::*;
        let path = [
            Queued,
            Running,
            AwaitingApproval,
            Queued,
            Running,
            Verifying,
            Succeeded,
        ];
        for pair in path.windows(2) {
            pair[0].transition(pair[1]).unwrap();
        }
    }

    #[test]
    fn interrupted_run_is_not_silently_requeued() {
        // Contract §4: on restart a Running run goes to reconciliation, it
        // does not jump back to Queued by itself.
        assert!(!RunState::Running.can_transition_to(RunState::Queued));
        assert!(RunState::Running.can_transition_to(RunState::NeedsReconciliation));
    }

    #[test]
    fn cancelled_run_interrupted_before_any_effect_can_end_cancelled() {
        RunState::CancellationRequested
            .transition(RunState::NeedsReconciliation)
            .unwrap()
            .transition(RunState::Cancelled)
            .unwrap();
    }

    #[test]
    fn approval_cannot_be_skipped_once_awaited() {
        assert!(!ActionState::AwaitingApproval.can_transition_to(ActionState::Executing));
        assert!(!RunState::AwaitingApproval.can_transition_to(RunState::Running));
    }
}
