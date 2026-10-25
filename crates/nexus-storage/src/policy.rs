//! The user's approval policy rules (ADR-POLICY-001, `schema::V21`): what
//! the user changed about a tool, within the fixed limits the Host checks
//! before writing. A rule is never deleted: a change revokes the rule in
//! force and writes the next revision, so the rule in force at any moment
//! (and who set it) can always be read back. A tool back to its default has
//! no rule in force.

use nexus_core::PolicyDecision;
use nexus_protocol::PolicyRuleId;
use rusqlite::{OptionalExtension, params};
use serde_json::json;

use crate::actors::{ActorRow, id_of, load};
use crate::repo::{insert_event, parse_state};
use crate::{Actor, StorageError, Store};

/// A rule in force.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRuleRow {
    pub tool_id: String,
    pub decision: PolicyDecision,
    /// For register writes: how many a reply makes without asking.
    pub max_auto: Option<u32>,
    pub revision: i64,
    pub changed_by: Option<ActorRow>,
    pub created_at_ms: i64,
}

impl Store {
    /// The rules in force, one per tool at most.
    pub async fn policy_rules(&self) -> Result<Vec<PolicyRuleRow>, StorageError> {
        self.call(|conn| {
            let raw: Vec<(String, String, Option<i64>, i64, String, i64)> = {
                let mut stmt = conn.prepare(
                    "SELECT tool_id, decision, max_auto, revision, changed_by, created_at_ms
                     FROM policy_rules WHERE revoked_at_ms IS NULL ORDER BY tool_id",
                )?;
                stmt.query_map([], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                })?
                .collect::<Result<_, _>>()?
            };
            raw.into_iter()
                .map(|(tool_id, decision, max_auto, revision, by, at)| {
                    Ok(PolicyRuleRow {
                        tool_id,
                        decision: parse_state(&decision)?,
                        max_auto: max_auto.map(|m| u32::try_from(m).unwrap_or(0)),
                        revision,
                        changed_by: load(conn, Some(&by))?,
                        created_at_ms: at,
                    })
                })
                .collect()
        })
        .await
    }

    /// Replaces the rule in force for `tool_id` with `rule` (`None`: back to
    /// the default), by `actor`. The caller has checked the fixed limits.
    pub async fn set_policy_rule(
        &self,
        tool_id: String,
        rule: Option<(PolicyDecision, Option<u32>)>,
        actor: Actor,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let last: Option<i64> = tx
                .query_row(
                    "SELECT max(revision) FROM policy_rules WHERE tool_id = ?1",
                    [&tool_id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            tx.execute(
                "UPDATE policy_rules SET revoked_at_ms = ?1 WHERE tool_id = ?2 AND revoked_at_ms IS NULL",
                params![now, tool_id],
            )?;
            let revision = last.unwrap_or(0) + 1;
            if let Some((decision, max_auto)) = rule {
                tx.execute(
                    "INSERT INTO policy_rules (id, tool_id, decision, max_auto, revision, changed_by, created_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        PolicyRuleId::new().to_string(),
                        tool_id,
                        decision.as_str(),
                        max_auto.map(i64::from),
                        revision,
                        id_of(&tx, actor)?,
                        now
                    ],
                )?;
            }
            insert_event(
                &tx,
                "PolicyChanged",
                &tool_id,
                revision,
                json!({
                    "tool": tool_id,
                    "decision": rule.map(|(d, _)| d.as_str()),
                    "max_auto": rule.and_then(|(_, m)| m),
                }),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
