//! Projects and memory on a real SQLite file (`SECURITATE_SI_VALIDARE_RO.md`
//! MEM-01): scope is respected in search and context, a proposal is decided
//! once, corrections and deletions keep the index coherent, and the closed
//! columns match the domain enums.

use std::path::PathBuf;

use nexus_core::{MemoryScope, MemorySource, MemoryStatus};
use nexus_protocol::ProjectId;
use nexus_storage::{MemoryFilter, NewExchange, NewMemory, SUPPORTED_VERSION, StorageError, Store};

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
    let dir = std::env::temp_dir().join(format!("nexus-memory-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

fn fact(
    scope: MemoryScope,
    project: Option<ProjectId>,
    content: &str,
    source: MemorySource,
) -> NewMemory {
    NewMemory {
        scope,
        project_id: project,
        content: content.to_string(),
        source,
        conversation_id: None,
        message_id: None,
        influences: Vec::new(),
    }
}

fn search(text: Option<&str>, project: Option<ProjectId>) -> MemoryFilter {
    MemoryFilter {
        text: text.map(str::to_string),
        project_id: project,
        limit: 50,
        ..MemoryFilter::default()
    }
}

#[tokio::test]
async fn a_proposal_is_decided_once_and_a_stale_edit_is_refused() {
    let env = env();
    let project = env.store.ensure_default_project(0).await.unwrap();

    let mine = env
        .store
        .add_memory(
            fact(
                MemoryScope::Personal,
                None,
                "Prefer short answers.",
                MemorySource::User,
            ),
            1,
        )
        .await
        .unwrap();
    assert_eq!(
        mine.status,
        MemoryStatus::Active,
        "the user's own fact is active at once"
    );

    let proposed = env
        .store
        .add_memory(
            NewMemory {
                influences: vec!["example.com".into()],
                ..fact(
                    MemoryScope::Project,
                    Some(project),
                    "The build uses pnpm.",
                    MemorySource::Agent,
                )
            },
            2,
        )
        .await
        .unwrap();
    assert_eq!(proposed.status, MemoryStatus::Proposed);
    assert_eq!(proposed.influences, vec!["example.com".to_string()]);
    assert_eq!(proposed.project_name.as_deref(), Some("Nexus"));

    // The user corrects it before approving; the approval must name the
    // revision they saw.
    let edited = env
        .store
        .edit_memory(
            proposed.id,
            proposed.revision,
            "The build uses pnpm 12.".into(),
            3,
        )
        .await
        .unwrap();
    assert!(edited.edited_by_user);
    assert!(matches!(
        env.store
            .decide_memory(proposed.id, proposed.revision, true, 4)
            .await,
        Err(StorageError::RevisionConflict(_))
    ));
    let approved = env
        .store
        .decide_memory(proposed.id, edited.revision, true, 5)
        .await
        .unwrap();
    assert_eq!(approved.status, MemoryStatus::Active);
    assert_eq!(approved.decided_at_ms, Some(5));
    assert!(matches!(
        env.store
            .decide_memory(proposed.id, approved.revision, false, 6)
            .await,
        Err(StorageError::IllegalTransition(_))
    ));

    let rejected = env
        .store
        .add_memory(
            fact(
                MemoryScope::Personal,
                None,
                "Lives on Mars.",
                MemorySource::Agent,
            ),
            7,
        )
        .await
        .unwrap();
    let rejected = env
        .store
        .decide_memory(rejected.id, rejected.revision, false, 8)
        .await
        .unwrap();
    assert_eq!(rejected.status, MemoryStatus::Rejected);
    let context = env.store.memory_context(project, 50, 10_000).await.unwrap();
    let texts: Vec<&str> = context.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(
        texts,
        vec!["The build uses pnpm 12.", "Prefer short answers."],
        "only active facts, newest first"
    );
}

#[tokio::test]
async fn mem01_search_and_context_respect_the_project_and_ignore_diacritics() {
    let env = env();
    let main = env.store.ensure_default_project(0).await.unwrap();
    let other = env
        .store
        .create_project("Side".into(), "a second workspace".into(), 1)
        .await
        .unwrap()
        .id;
    for (scope, project, text) in [
        (MemoryScope::Personal, None, "Notă: scrie în română."),
        (
            MemoryScope::Project,
            Some(main),
            "Notă internă a proiectului principal.",
        ),
        (
            MemoryScope::Project,
            Some(other),
            "Secretul proiectului secundar.",
        ),
    ] {
        env.store
            .add_memory(fact(scope, project, text, MemorySource::User), 2)
            .await
            .unwrap();
    }

    let in_main = env
        .store
        .search_memory(search(Some("nota"), Some(main)))
        .await
        .unwrap();
    assert_eq!(
        in_main.len(),
        2,
        "\"nota\" finds \"Notă\" in the project and in personal memory"
    );
    let in_other = env
        .store
        .search_memory(search(Some("nota"), Some(other)))
        .await
        .unwrap();
    assert_eq!(in_other.len(), 1, "another project's fact never shows up");
    assert_eq!(in_other[0].scope, MemoryScope::Personal);
    assert!(
        env.store
            .search_memory(search(Some("secretul"), Some(main)))
            .await
            .unwrap()
            .is_empty()
    );
    let context = env.store.memory_context(other, 50, 10_000).await.unwrap();
    assert!(context.iter().all(|m| !m.content.contains("principal")));
    assert!(
        env.store
            .search_memory(search(Some("\" OR content NEAR("), None))
            .await
            .unwrap()
            .is_empty(),
        "search syntax typed by the user is matched as words, never executed"
    );

    // A correction and a deletion keep the index coherent.
    let fact = env
        .store
        .search_memory(search(Some("secretul"), Some(other)))
        .await
        .unwrap()
        .remove(0);
    env.store
        .edit_memory(
            fact.id,
            fact.revision,
            "Cheia de build a proiectului secundar.".into(),
            3,
        )
        .await
        .unwrap();
    assert!(
        env.store
            .search_memory(search(Some("secretul"), None))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        env.store
            .search_memory(search(Some("cheia"), None))
            .await
            .unwrap()
            .len(),
        1
    );
    env.store.delete_memory(fact.id, 4).await.unwrap();
    assert!(
        env.store
            .search_memory(search(Some("cheia"), None))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        env.store.memory(fact.id).await,
        Err(StorageError::NotFound(_))
    ));
}

#[tokio::test]
async fn the_context_is_bounded_by_count_and_size() {
    let env = env();
    let project = env.store.ensure_default_project(0).await.unwrap();
    for i in 0..10 {
        env.store
            .add_memory(
                fact(
                    MemoryScope::Personal,
                    None,
                    &format!("fact number {i:02} {}", "x".repeat(80)),
                    MemorySource::User,
                ),
                i,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        env.store
            .memory_context(project, 3, 10_000)
            .await
            .unwrap()
            .len(),
        3
    );
    let sized = env.store.memory_context(project, 50, 300).await.unwrap();
    assert_eq!(sized.len(), 3, "each fact is ~95 characters");
    assert!(
        sized[0].content.starts_with("fact number 09"),
        "newest first"
    );
}

#[tokio::test]
async fn projects_are_explicit_and_the_default_one_takes_new_tasks_until_another_is_chosen() {
    let env = env();
    let main = env.store.ensure_default_project(0).await.unwrap();
    assert_eq!(env.store.active_project().await.unwrap(), main);
    let side = env
        .store
        .create_project("Side".into(), String::new(), 1)
        .await
        .unwrap();
    assert_eq!(
        env.store.active_project().await.unwrap(),
        main,
        "creating is not choosing"
    );
    env.store.set_active_project(side.id, 2).await.unwrap();
    assert_eq!(env.store.active_project().await.unwrap(), side.id);

    env.store.persona(0).await.unwrap();
    let connection = env
        .store
        .save_connection(
            None,
            "Gemini",
            "g".into(),
            false,
            "gemini-mem-test".into(),
            3,
        )
        .await
        .unwrap()
        .id;
    let sent = env
        .store
        .append_exchange(
            NewExchange {
                conversation_id: None,
                project_id: Some(side.id),
                idempotency_key: "p1".into(),
                text: "Salut".into(),
                connection_id: connection,
                model: "m".into(),
                persona_version: 1,
            },
            4,
        )
        .await
        .unwrap();
    assert_eq!(
        env.store
            .conversation_project(sent.conversation_id)
            .await
            .unwrap(),
        side.id
    );
    let rows = env.store.projects().await.unwrap();
    assert_eq!(
        rows.iter().map(|p| p.conversations).collect::<Vec<_>>(),
        vec![0, 1]
    );

    let renamed = env
        .store
        .update_project(side.id, "Side project".into(), "notes".into(), 5)
        .await
        .unwrap();
    assert_eq!(
        (renamed.name.as_str(), renamed.revision),
        ("Side project", 2)
    );
    assert!(
        env.store
            .set_active_project(ProjectId::new(), 6)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn memory_columns_accept_exactly_the_domain_values() {
    let env = env();
    let project = env
        .store
        .ensure_default_project(0)
        .await
        .unwrap()
        .to_string();
    assert_eq!(env.store.schema_version().await.unwrap(), SUPPORTED_VERSION);
    let refused = env
        .store
        .call(move |c| {
            let mut seq = 0;
            for scope in MemoryScope::ALL {
                for status in MemoryStatus::ALL {
                    for source in MemorySource::ALL {
                        seq += 1;
                        let project = (*scope == MemoryScope::Project).then_some(project.clone());
                        let ok = c.execute(
                            "INSERT INTO memory_items (id, scope, project_id, content, status, source, revision,
                             created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, 'x', ?4, ?5, 1, 0, 0)",
                            (format!("m{seq}"), scope.as_str(), project, status.as_str(), source.as_str()),
                        );
                        // Only the agent's facts can wait or be rejected.
                        let allowed = *source == MemorySource::Agent || *status == MemoryStatus::Active;
                        assert_eq!(ok.is_ok(), allowed, "{scope:?}/{status:?}/{source:?}");
                    }
                }
            }
            let mut refused = Vec::new();
            for (name, sql) in [
                ("scope", "INSERT INTO memory_items VALUES ('a','Team',NULL,'x','Active','User',0,NULL,NULL,'[]',1,0,0,NULL)"),
                ("status", "INSERT INTO memory_items VALUES ('b','Personal',NULL,'x','Forgotten','Agent',0,NULL,NULL,'[]',1,0,0,NULL)"),
                ("project without id", "INSERT INTO memory_items VALUES ('c','Project',NULL,'x','Active','User',0,NULL,NULL,'[]',1,0,0,NULL)"),
                ("empty", "INSERT INTO memory_items VALUES ('d','Personal',NULL,'','Active','User',0,NULL,NULL,'[]',1,0,0,NULL)"),
            ] {
                if c.execute(sql, []).is_err() {
                    refused.push(name);
                }
            }
            Ok(refused)
        })
        .await
        .unwrap();
    assert_eq!(
        refused,
        vec!["scope", "status", "project without id", "empty"]
    );
}
