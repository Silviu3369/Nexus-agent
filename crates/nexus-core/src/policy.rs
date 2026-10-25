//! The approval policy (ADR-POLICY-001): what the Host decides for each
//! call of a tool before anything happens. Fixed limits, in code, bound
//! what a rule may change: an effect outside Nexus is always asked for (or
//! refused), a read never needs asking, a fact for memory or a note is
//! always asked for; only the agent's register writes may run without
//! asking. The model, a document or a pack cannot relax anything.

use crate::state::{EffectClass, PolicyDecision};

/// The revision of the policy built into this version, recorded with what
/// it decided (a rule the user sets will have its own).
pub const BUILT_IN_REVISION: i64 = 1;

/// What the policy knows about a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolPolicy {
    pub class: EffectClass,
    /// The tool asks although its class would not: a fact for memory, a
    /// note.
    pub asks: bool,
}

/// What the tool does when no rule says otherwise.
pub fn default_decision(tool: ToolPolicy) -> PolicyDecision {
    match tool.class {
        EffectClass::Read => PolicyDecision::Auto,
        EffectClass::InternalReversible if !tool.asks => PolicyDecision::Auto,
        _ => PolicyDecision::Ask,
    }
}

/// Whether a rule may set `decision` for the tool: anything may be
/// refused; asking makes no sense for a read; only what runs by default
/// without asking (a register write) may keep doing so.
pub fn permits(tool: ToolPolicy, decision: PolicyDecision) -> bool {
    match decision {
        PolicyDecision::Deny => true,
        PolicyDecision::Ask => tool.class != EffectClass::Read,
        PolicyDecision::Auto => default_decision(tool) == PolicyDecision::Auto,
    }
}

/// An approval of an effect outside Nexus must come from the native Windows
/// dialog, not from the window that shows the model's and documents'
/// content: an IPC call does not prove a click of the user (§11). An effect
/// of unknown class is treated as outside.
pub fn needs_trusted_confirmation(class: Option<EffectClass>) -> bool {
    !matches!(
        class,
        Some(EffectClass::Read | EffectClass::InternalReversible)
    )
}

/// The decision for one call: the rule's, when the limits permit it,
/// otherwise the tool's default.
pub fn decide(tool: ToolPolicy, rule: Option<PolicyDecision>) -> PolicyDecision {
    match rule {
        Some(decision) if permits(tool, decision) => decision,
        _ => default_decision(tool),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READ: ToolPolicy = ToolPolicy {
        class: EffectClass::Read,
        asks: false,
    };
    const RECORD_WRITE: ToolPolicy = ToolPolicy {
        class: EffectClass::InternalReversible,
        asks: false,
    };
    const MEMORY_FACT: ToolPolicy = ToolPolicy {
        class: EffectClass::InternalReversible,
        asks: true,
    };
    const DOCUMENT: ToolPolicy = ToolPolicy {
        class: EffectClass::ExternalReversible,
        asks: true,
    };
    const EMAIL: ToolPolicy = ToolPolicy {
        class: EffectClass::ExternalIrreversible,
        asks: true,
    };

    #[test]
    fn the_defaults_are_the_behaviour_before_rules() {
        assert_eq!(default_decision(READ), PolicyDecision::Auto);
        assert_eq!(default_decision(RECORD_WRITE), PolicyDecision::Auto);
        assert_eq!(default_decision(MEMORY_FACT), PolicyDecision::Ask);
        assert_eq!(default_decision(DOCUMENT), PolicyDecision::Ask);
        assert_eq!(default_decision(EMAIL), PolicyDecision::Ask);
        // An external tool asks even if its descriptor forgot to say so.
        let careless = ToolPolicy {
            class: EffectClass::ExternalIrreversible,
            asks: false,
        };
        assert_eq!(default_decision(careless), PolicyDecision::Ask);
    }

    #[test]
    fn a_rule_can_only_tighten_except_for_register_writes() {
        use PolicyDecision::*;
        // The table of ADR-POLICY-001: (tool, Auto, Ask, Deny) permitted.
        let table = [
            (READ, true, false, true),
            (RECORD_WRITE, true, true, true),
            (MEMORY_FACT, false, true, true),
            (DOCUMENT, false, true, true),
            (EMAIL, false, true, true),
        ];
        for (tool, auto, ask, deny) in table {
            assert_eq!(permits(tool, Auto), auto, "{tool:?} Auto");
            assert_eq!(permits(tool, Ask), ask, "{tool:?} Ask");
            assert_eq!(permits(tool, Deny), deny, "{tool:?} Deny");
        }
    }

    #[test]
    fn only_an_effect_outside_nexus_needs_the_windows_dialog() {
        assert!(!needs_trusted_confirmation(Some(EffectClass::Read)));
        assert!(!needs_trusted_confirmation(Some(
            EffectClass::InternalReversible
        )));
        assert!(needs_trusted_confirmation(Some(
            EffectClass::ExternalReversible
        )));
        assert!(needs_trusted_confirmation(Some(
            EffectClass::ExternalIrreversible
        )));
        assert!(
            needs_trusted_confirmation(None),
            "unknown counts as outside"
        );
    }

    #[test]
    fn a_rule_the_limits_do_not_permit_is_ignored() {
        use PolicyDecision::*;
        assert_eq!(
            decide(EMAIL, Some(Auto)),
            Ask,
            "an email is never sent unasked"
        );
        assert_eq!(decide(DOCUMENT, Some(Auto)), Ask);
        assert_eq!(decide(MEMORY_FACT, Some(Auto)), Ask);
        assert_eq!(decide(READ, Some(Ask)), Auto);
        assert_eq!(decide(EMAIL, Some(Deny)), Deny, "anything can be refused");
        assert_eq!(decide(RECORD_WRITE, Some(Ask)), Ask);
        assert_eq!(decide(RECORD_WRITE, None), Auto);
    }
}
