//! The usage ledger, daily budget and prices (`schema::V9`) on a real
//! SQLite file: tokens are summed from a moment on and per model, the
//! budget and prices are set and cleared, values the schema forbids are
//! refused, and the migration keeps every message.

use std::path::PathBuf;

use nexus_storage::{NewExchange, SUPPORTED_VERSION, Store, UsageRow};

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
    let dir = std::env::temp_dir().join(format!("nexus-usage-test-{}", uuid::Uuid::new_v4()));
    Env {
        store: Store::open(&dir.join("nexus.db")).unwrap(),
        dir,
    }
}

#[tokio::test]
async fn the_ledger_sums_tokens_from_a_moment_on_and_per_model() {
    let env = env();
    let s = &env.store;
    s.record_model_usage("Gemini".into(), "flash".into(), true, 100, 20, 1_000)
        .await
        .unwrap();
    s.record_model_usage("Gemini".into(), "flash".into(), true, 50, 10, 2_000)
        .await
        .unwrap();
    s.record_model_usage("OpenAI".into(), "gpt".into(), false, 7, 3, 3_000)
        .await
        .unwrap();
    assert_eq!(s.tokens_since(0).await.unwrap(), 190);
    assert_eq!(
        s.tokens_since(1_500).await.unwrap(),
        70,
        "earlier calls do not count"
    );
    assert_eq!(
        s.usage_since(0).await.unwrap(),
        [
            UsageRow {
                provider: "Gemini".into(),
                model: "flash".into(),
                free_tier: true,
                calls: 2,
                prompt_tokens: 150,
                output_tokens: 30,
            },
            UsageRow {
                provider: "OpenAI".into(),
                model: "gpt".into(),
                free_tier: false,
                calls: 1,
                prompt_tokens: 7,
                output_tokens: 3,
            },
        ]
    );
    let kept: i64 = s
        .call(|c| Ok(c.query_row("SELECT count(*) FROM model_usage", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(kept, 3);
    // Ninety days later the oldest rows are pruned as new ones arrive.
    let later = 3_000 + 91 * 24 * 60 * 60 * 1000;
    s.record_model_usage("OpenAI".into(), "gpt".into(), false, 1, 1, later)
        .await
        .unwrap();
    assert_eq!(s.tokens_since(0).await.unwrap(), 2);
}

#[tokio::test]
async fn the_budget_and_prices_are_set_cleared_and_checked() {
    let env = env();
    let s = &env.store;
    assert_eq!(s.daily_token_budget().await.unwrap(), None);
    s.set_daily_token_budget(Some(50_000), 1).await.unwrap();
    assert_eq!(s.daily_token_budget().await.unwrap(), Some(50_000));
    s.set_daily_token_budget(None, 2).await.unwrap();
    assert_eq!(s.daily_token_budget().await.unwrap(), None);

    s.set_model_price("OpenAI".into(), "gpt".into(), Some((1.25, 10.0)), 3)
        .await
        .unwrap();
    s.set_model_price("OpenAI".into(), "gpt".into(), Some((2.0, 8.0)), 4)
        .await
        .unwrap();
    let prices = s.model_prices().await.unwrap();
    assert_eq!(prices.len(), 1, "one price per model");
    assert_eq!(
        (prices[0].input_per_million, prices[0].output_per_million),
        (2.0, 8.0)
    );
    assert!(
        s.set_model_price("OpenAI".into(), "gpt".into(), Some((-1.0, 1.0)), 5)
            .await
            .is_err(),
        "the schema refuses a negative price"
    );
    s.set_model_price("OpenAI".into(), "gpt".into(), None, 6)
        .await
        .unwrap();
    assert!(s.model_prices().await.unwrap().is_empty());
    let events = s.events_since(0, 100).await.unwrap();
    assert!(events.iter().any(|e| e.kind == "DailyTokenBudgetSet"));
    assert!(events.iter().any(|e| e.kind == "ModelPriceSet"));
}

#[tokio::test]
async fn migrating_to_v9_keeps_every_message() {
    let dir = std::env::temp_dir().join(format!("nexus-migrate-v9-test-{}", uuid::Uuid::new_v4()));
    let path = dir.join("nexus.db");
    let conversation = {
        let store = Store::open(&path).unwrap();
        store.persona(0).await.unwrap();
        store.ensure_default_project(0).await.unwrap();
        let connection = store
            .save_connection(None, "Gemini", "g".into(), false, "gemini-v9".into(), 1)
            .await
            .unwrap()
            .id;
        let ex = store
            .append_exchange(
                NewExchange {
                    conversation_id: None,
                    project_id: None,
                    idempotency_key: "m1".into(),
                    text: "Salut".into(),
                    connection_id: connection,
                    model: "m".into(),
                    persona_version: 1,
                },
                2,
            )
            .await
            .unwrap();
        store
            .call(|c| {
                c.execute_batch(
                    "DROP TABLE model_usage; DROP TABLE model_prices; PRAGMA user_version = 8;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        ex.conversation_id
    };
    let store = Store::open(&path).unwrap();
    assert_eq!(store.schema_version().await.unwrap(), SUPPORTED_VERSION);
    assert_eq!(store.messages(conversation, 10).await.unwrap().1.len(), 2);
    assert_eq!(store.tokens_since(0).await.unwrap(), 0);
    drop(store);
    let _ = std::fs::remove_dir_all(dir);
}
