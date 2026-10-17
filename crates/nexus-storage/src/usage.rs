//! Tokens spent per model call, the daily budget and the prices the user
//! enters (`schema::V9`). The ledger keeps numbers only — provider, model,
//! tokens, time — so deleting a task removes its text but not the record
//! that its tokens were spent (the budget cannot be reset by deleting).

use rusqlite::{OptionalExtension, params};
use serde_json::json;

use crate::repo::insert_event;
use crate::{StorageError, Store};

const DAILY_TOKEN_BUDGET: &str = "daily_token_budget";
/// The ledger keeps this long (only today counts toward the budget).
const KEEP_MS: i64 = 90 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub provider: String,
    pub model: String,
    pub free_tier: bool,
    pub calls: u32,
    pub prompt_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PriceRow {
    pub provider: String,
    pub model: String,
    pub input_per_million: f64,
    pub output_per_million: f64,
}

impl Store {
    /// One model call's tokens, as the provider reported them.
    pub async fn record_model_usage(
        &self,
        provider: String,
        model: String,
        free_tier: bool,
        prompt_tokens: u32,
        output_tokens: u32,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO model_usage (at_ms, provider, model, free_tier, prompt_tokens, output_tokens)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![now, provider, model, free_tier, prompt_tokens, output_tokens],
            )?;
            tx.execute("DELETE FROM model_usage WHERE at_ms < ?1", [now - KEEP_MS])?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Tokens (sent and received) recorded since `since` (unix ms).
    pub async fn tokens_since(&self, since: i64) -> Result<u64, StorageError> {
        self.call(move |conn| {
            let total: i64 = conn.query_row(
                "SELECT coalesce(sum(prompt_tokens + output_tokens), 0) FROM model_usage WHERE at_ms >= ?1",
                [since],
                |r| r.get(0),
            )?;
            Ok(u64::try_from(total).unwrap_or(0))
        })
        .await
    }

    /// Per provider and model since `since`.
    pub async fn usage_since(&self, since: i64) -> Result<Vec<UsageRow>, StorageError> {
        self.call(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT provider, model, max(free_tier), count(*), sum(prompt_tokens), sum(output_tokens)
                 FROM model_usage WHERE at_ms >= ?1 GROUP BY provider, model ORDER BY provider, model",
            )?;
            let rows = stmt
                .query_map([since], |r| {
                    Ok(UsageRow {
                        provider: r.get(0)?,
                        model: r.get(1)?,
                        free_tier: r.get(2)?,
                        calls: r.get(3)?,
                        prompt_tokens: u64::try_from(r.get::<_, i64>(4)?).unwrap_or(0),
                        output_tokens: u64::try_from(r.get::<_, i64>(5)?).unwrap_or(0),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    pub async fn daily_token_budget(&self) -> Result<Option<u64>, StorageError> {
        self.call(|conn| {
            let value: Option<i64> = conn
                .query_row(
                    "SELECT json_extract(value_json, '$') FROM settings WHERE key = ?1",
                    [DAILY_TOKEN_BUDGET],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(value.and_then(|v| u64::try_from(v).ok()))
        })
        .await
    }

    /// `None` removes the limit.
    pub async fn set_daily_token_budget(
        &self,
        tokens: Option<u64>,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            match tokens {
                Some(t) => {
                    tx.execute(
                        "INSERT INTO settings (key, value_json) VALUES (?1, ?2)
                         ON CONFLICT (key) DO UPDATE SET value_json = excluded.value_json",
                        params![DAILY_TOKEN_BUDGET, json!(t).to_string()],
                    )?;
                }
                None => {
                    tx.execute("DELETE FROM settings WHERE key = ?1", [DAILY_TOKEN_BUDGET])?;
                }
            }
            insert_event(
                &tx,
                "DailyTokenBudgetSet",
                "usage",
                0,
                json!({ "tokens": tokens }),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn model_prices(&self) -> Result<Vec<PriceRow>, StorageError> {
        self.call(|conn| {
            let mut stmt = conn.prepare(
                "SELECT provider, model, input_per_million, output_per_million FROM model_prices
                 ORDER BY provider, model",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(PriceRow {
                        provider: r.get(0)?,
                        model: r.get(1)?,
                        input_per_million: r.get(2)?,
                        output_per_million: r.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// `None` forgets the model's price.
    pub async fn set_model_price(
        &self,
        provider: String,
        model: String,
        price: Option<(f64, f64)>,
        now: i64,
    ) -> Result<(), StorageError> {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            match price {
                Some((input, output)) => {
                    tx.execute(
                        "INSERT INTO model_prices (provider, model, input_per_million, output_per_million, updated_at_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT (provider, model) DO UPDATE SET input_per_million = excluded.input_per_million,
                           output_per_million = excluded.output_per_million, updated_at_ms = excluded.updated_at_ms",
                        params![provider, model, input, output, now],
                    )?;
                }
                None => {
                    tx.execute(
                        "DELETE FROM model_prices WHERE provider = ?1 AND model = ?2",
                        params![provider, model],
                    )?;
                }
            }
            insert_event(
                &tx,
                "ModelPriceSet",
                &format!("{provider}/{model}"),
                0,
                json!({ "provider": provider, "model": model, "set": price.is_some() }),
                now,
            )?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
