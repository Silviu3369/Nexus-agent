//! The user's approval policy as the Host applies it (ADR-POLICY-001): the
//! rules the user set, read once per reply, within the fixed limits of
//! `nexus_core::policy`. A tool turned off is never offered to the model and
//! is refused if asked for anyway; register writes beyond the user's number
//! wait for the user in one batch. Loosening (a tool back on, more writes
//! without asking) is accepted only from the native Windows dialog.

use std::collections::HashMap;

use nexus_core::PolicyDecision;
use nexus_core::policy::{self, MAX_BATCH_ITEMS, RECORD_WRITES_PER_TURN};
use nexus_protocol::{ApprovalPath, ErrorCode, NexusError, PermissionView, SetPermissionRequest};
use nexus_storage::{Actor, PolicyRuleRow, now_ms};

use super::{Runtime, to_nexus_error};
use crate::tools::registry::{self, RECORD_WRITE_ID};

/// The rules in force for one reply.
#[derive(Debug, Default)]
pub(super) struct Permissions {
    rules: HashMap<String, PolicyRuleRow>,
}

impl Permissions {
    fn from_rules(rules: Vec<PolicyRuleRow>) -> Self {
        Self {
            rules: rules.into_iter().map(|r| (r.tool_id.clone(), r)).collect(),
        }
    }

    /// The tool may be offered and used: the user did not turn it off.
    pub fn allows(&self, tool_id: &str) -> bool {
        self.rules
            .get(tool_id)
            .is_none_or(|r| r.decision != PolicyDecision::Deny)
    }

    /// How many register writes a reply makes without asking.
    pub fn record_threshold(&self) -> u32 {
        self.rules
            .get(RECORD_WRITE_ID)
            .and_then(|r| r.max_auto)
            .unwrap_or(RECORD_WRITES_PER_TURN)
    }

    /// The most register writes a reply makes, with the batch.
    pub fn max_record_writes(&self) -> u32 {
        self.record_threshold() + MAX_BATCH_ITEMS as u32
    }
}

fn invalid(message: &str) -> NexusError {
    NexusError::new(ErrorCode::InvalidRequest, message.to_string())
}

impl Runtime {
    /// The rules in force, for one reply.
    pub(super) async fn permissions_in_force(&self) -> Result<Permissions, NexusError> {
        let rules = self.store.policy_rules().await.map_err(to_nexus_error)?;
        Ok(Permissions::from_rules(rules))
    }

    /// Every tool the model can be offered, with what the policy lets it do.
    pub async fn permissions(&self) -> Result<Vec<PermissionView>, NexusError> {
        let rules = self.store.policy_rules().await.map_err(to_nexus_error)?;
        let in_force = Permissions::from_rules(rules);
        Ok(registry::TOOLS
            .iter()
            .filter(|t| registry::offered_to_model(t))
            .map(|t| {
                let writes = t.id == RECORD_WRITE_ID;
                PermissionView {
                    tool_id: t.id.to_string(),
                    label: t.label.to_string(),
                    effect_class: t.effect.as_str().to_string(),
                    sends: t.sends.to_string(),
                    enabled: in_force.allows(t.id),
                    decision: policy::default_decision(t.policy()).as_str().to_string(),
                    max_auto: writes.then(|| in_force.record_threshold()),
                    default_max_auto: writes.then_some(RECORD_WRITES_PER_TURN),
                    changed_at_ms: in_force.rules.get(t.id).map(|r| r.created_at_ms),
                }
            })
            .collect())
    }

    /// Changes one tool within the fixed limits. The page cannot loosen the
    /// policy: only the app's own dialog can.
    pub async fn set_permission(
        &self,
        request: SetPermissionRequest,
    ) -> Result<Vec<PermissionView>, NexusError> {
        let tool = registry::by_id(&request.tool_id)
            .filter(|t| registry::offered_to_model(t))
            .ok_or_else(|| invalid("no such tool can be changed"))?;
        let writes = tool.id == RECORD_WRITE_ID;
        let max_auto = match (writes, request.max_auto) {
            (true, Some(n)) if n as usize <= MAX_BATCH_ITEMS => Some(n),
            (true, Some(_)) => {
                return Err(invalid(&format!(
                    "a reply makes at most {MAX_BATCH_ITEMS} register writes without asking"
                )));
            }
            (true, None) => Some(RECORD_WRITES_PER_TURN),
            (false, Some(_)) => {
                return Err(invalid("only register writes have a number to set"));
            }
            (false, None) => None,
        };
        let in_force = self.permissions_in_force().await?;
        let loosens = (request.enabled && !in_force.allows(tool.id))
            || (writes && request.enabled && max_auto > Some(in_force.record_threshold()));
        if loosens && request.via != ApprovalPath::TrustedDialog {
            return Err(NexusError::new(
                ErrorCode::Forbidden,
                "turning a tool back on, or letting more run without asking, is confirmed in the Windows dialog",
            ));
        }
        let rule = if !request.enabled {
            Some((PolicyDecision::Deny, None))
        } else if writes && max_auto != Some(RECORD_WRITES_PER_TURN) {
            Some((PolicyDecision::Auto, max_auto))
        } else {
            None
        };
        if let Some((decision, _)) = rule
            && !policy::permits(tool.policy(), decision)
        {
            return Err(invalid("the fixed limits do not allow that"));
        }
        self.store
            .set_policy_rule(tool.id.to_string(), rule, Actor::LocalUser, now_ms())
            .await
            .map_err(to_nexus_error)?;
        self.permissions().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(tool: &str, decision: PolicyDecision, max_auto: Option<u32>) -> PolicyRuleRow {
        PolicyRuleRow {
            tool_id: tool.into(),
            decision,
            max_auto,
            revision: 1,
            changed_by: None,
            created_at_ms: 0,
        }
    }

    #[test]
    fn the_defaults_hold_until_a_rule_says_otherwise() {
        let defaults = Permissions::default();
        assert!(defaults.allows("WebRead"));
        assert_eq!(defaults.record_threshold(), 10);
        assert_eq!(defaults.max_record_writes(), 35);

        let set = Permissions::from_rules(vec![
            rule("WebRead", PolicyDecision::Deny, None),
            rule(RECORD_WRITE_ID, PolicyDecision::Auto, Some(0)),
        ]);
        assert!(!set.allows("WebRead"));
        assert!(set.allows(RECORD_WRITE_ID));
        assert_eq!(set.record_threshold(), 0, "every write asks");
        assert_eq!(set.max_record_writes(), 25);
    }
}
