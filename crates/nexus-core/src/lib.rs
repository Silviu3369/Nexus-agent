//! Domain core: task, run, step, action, approval and policy state, kept
//! independent of Tauri, React, SQLite drivers and provider SDKs.

pub mod policy;
pub mod state;

pub use state::{
    ActionState, ActorKind, AgentStepKind, AgentStepStatus, DecisionPath, EffectClass, Evidence,
    FailureCause, IllegalTransition, InterventionKind, MemoryScope, MemorySource, MemoryStatus,
    MessageRole, MessageStatus, OperationState, PolicyDecision, RecordActor, RecordChangeKind,
    RunState, RunType, TaskOrigin, ThinkingLevel, UnknownState,
};
