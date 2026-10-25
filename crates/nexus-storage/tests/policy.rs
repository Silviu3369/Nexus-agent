//! The user's approval policy rules (`schema::V21`, ADR-POLICY-001) on a
//! real SQLite file: one rule in force per tool, each change a new revision
//! by its actor, the earlier rule kept (revoked), and back to the default
//! leaves no rule in force.

mod common;

use std::path::PathBuf;

use nexus_core::{ActorKind, PolicyDecision};
use nexus_storage::{Actor, SUPPORTED_VERSION, Store};

struct Env {
    store: Store,
    dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn env() -> Env {
    let dir = std::env::temp_dir().join(format!("nexus-policy-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

async fn count(store: &Store, sql: &'static str) -> i64 {
    store
        .call(move |c| Ok(c.query_row(sql, [], |r| r.get(0))?))
        .await
        .unwrap()
}

#[tokio::test]
async fn each_change_is_a_new_revision_and_the_old_rule_is_kept() {
    let env = env();
    let s = &env.store;
    assert!(
        s.policy_rules().await.unwrap().is_empty(),
        "the defaults hold"
    );

    s.set_policy_rule(
        "WebRead".into(),
        Some((PolicyDecision::Deny, None)),
        Actor::LocalUser,
        10,
    )
    .await
    .unwrap();
    s.set_policy_rule(
        "RecordWrite".into(),
        Some((PolicyDecision::Auto, Some(3))),
        Actor::LocalUser,
        11,
    )
    .await
    .unwrap();
    let rules = s.policy_rules().await.unwrap();
    assert_eq!(
        rules
            .iter()
            .map(|r| (r.tool_id.as_str(), r.decision, r.max_auto, r.revision))
            .collect::<Vec<_>>(),
        [
            ("RecordWrite", PolicyDecision::Auto, Some(3), 1),
            ("WebRead", PolicyDecision::Deny, None, 1),
        ]
    );
    assert_eq!(
        rules[0].changed_by.as_ref().map(|a| a.kind),
        Some(ActorKind::User)
    );

    // Changed again, then back to the default.
    s.set_policy_rule(
        "RecordWrite".into(),
        Some((PolicyDecision::Auto, Some(0))),
        Actor::LocalUser,
        12,
    )
    .await
    .unwrap();
    s.set_policy_rule("WebRead".into(), None, Actor::LocalUser, 13)
        .await
        .unwrap();
    let rules = s.policy_rules().await.unwrap();
    assert_eq!(rules.len(), 1, "web reading is back to its default");
    assert_eq!((rules[0].max_auto, rules[0].revision), (Some(0), 2));
    assert_eq!(
        count(s, "SELECT count(*) FROM policy_rules").await,
        3,
        "no rule is deleted"
    );
    assert_eq!(
        count(
            s,
            "SELECT count(*) FROM events WHERE kind = 'PolicyChanged'"
        )
        .await,
        4
    );
}

#[tokio::test]
async fn migrating_to_v21_starts_with_the_defaults() {
    let env = env();
    let path = env.dir.join("nexus.db");
    env.store
        .call(|c| {
            common::undo_v21(c)?;
            Ok(())
        })
        .await
        .unwrap();
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().await.unwrap(), SUPPORTED_VERSION);
    assert!(s.policy_rules().await.unwrap().is_empty());
}
