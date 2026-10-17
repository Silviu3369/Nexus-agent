//! Tokens per day, the user's daily budget and cost estimates
//! (`PLAN_NEXUS_RO.md` M2). Usage is what the providers reported for each
//! model call; a cost is an estimate from prices the user entered, shown
//! separately and never presented as a bill. A free tier is not charged.

use nexus_protocol::{ErrorCode, ModelPrice, ModelUsageView, NexusError, UsageView};
use nexus_storage::now_ms;
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

use super::{Runtime, to_nexus_error};

/// The largest daily budget accepted (well beyond any real day).
const MAX_BUDGET: u64 = 1_000_000_000_000;
/// The largest price accepted, in US dollars per million tokens.
const MAX_PRICE: f64 = 10_000.0;

/// Start of today in this PC's local time, as unix ms. On the day a clock
/// change happens it may be off by that hour.
pub(crate) fn local_day_start_ms(now: i64) -> i64 {
    let mut local = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    // SAFETY: GetLocalTime only writes the structure it is given.
    unsafe { GetLocalTime(&mut local) };
    let since_midnight = ((i64::from(local.wHour) * 60 + i64::from(local.wMinute)) * 60
        + i64::from(local.wSecond))
        * 1000
        + i64::from(local.wMilliseconds);
    now - since_midnight
}

fn cost(prompt: u64, output: u64, price: &ModelPrice) -> f64 {
    (prompt as f64 * price.input_per_million + output as f64 * price.output_per_million)
        / 1_000_000.0
}

impl Runtime {
    /// Refused once today's tokens reach the user's budget; checked before
    /// every model call, so a reply stops between steps rather than run on.
    pub(crate) async fn check_daily_budget(&self) -> Result<(), NexusError> {
        let Some(budget) = self
            .store
            .daily_token_budget()
            .await
            .map_err(to_nexus_error)?
        else {
            return Ok(());
        };
        let used = self
            .store
            .tokens_since(local_day_start_ms(now_ms()))
            .await
            .map_err(to_nexus_error)?;
        if used >= budget {
            return Err(NexusError::new(
                ErrorCode::BudgetExceeded,
                format!(
                    "the daily budget of {} is used up ({used} used today); raise it in Models or wait until tomorrow",
                    if budget == 1 {
                        "1 token".to_string()
                    } else {
                        format!("{budget} tokens")
                    }
                ),
            ));
        }
        Ok(())
    }

    pub async fn usage(&self) -> Result<UsageView, NexusError> {
        let now = now_ms();
        let day_start_ms = local_day_start_ms(now);
        let used = self
            .store
            .usage_since(day_start_ms)
            .await
            .map_err(to_nexus_error)?;
        let prices = self.store.model_prices().await.map_err(to_nexus_error)?;
        let connections = self.store.connections().await.map_err(to_nexus_error)?;
        let backup = self.store.backup_model().await.map_err(to_nexus_error)?;

        // Every model worth showing: used today, chosen, or priced.
        let mut models: Vec<ModelUsageView> = Vec::new();
        let mut add = |provider: &str, model: &str, free_tier: bool| {
            if model.is_empty() {
                return;
            }
            match models
                .iter_mut()
                .find(|m| m.provider == provider && m.model == model)
            {
                Some(m) => m.free_tier |= free_tier,
                None => models.push(ModelUsageView {
                    provider: provider.to_string(),
                    model: model.to_string(),
                    calls_today: 0,
                    prompt_tokens_today: 0,
                    output_tokens_today: 0,
                    free_tier,
                    price: None,
                    estimated_cost_today: None,
                }),
            }
        };
        for row in &used {
            add(&row.provider, &row.model, row.free_tier);
        }
        for c in &connections {
            if let Some(model) = &c.model {
                add(&c.provider, model, c.free_tier);
            }
        }
        if let Some((row, model)) = &backup {
            add(&row.provider, model, row.free_tier);
        }
        for p in &prices {
            add(&p.provider, &p.model, false);
        }
        for m in &mut models {
            if let Some(row) = used
                .iter()
                .find(|r| r.provider == m.provider && r.model == m.model)
            {
                m.calls_today = row.calls;
                m.prompt_tokens_today = row.prompt_tokens;
                m.output_tokens_today = row.output_tokens;
            }
            m.price = prices
                .iter()
                .find(|p| p.provider == m.provider && p.model == m.model)
                .map(|p| ModelPrice {
                    input_per_million: p.input_per_million,
                    output_per_million: p.output_per_million,
                });
            m.estimated_cost_today = if m.free_tier {
                Some(0.0)
            } else {
                m.price
                    .as_ref()
                    .map(|p| cost(m.prompt_tokens_today, m.output_tokens_today, p))
            };
        }
        let estimates: Vec<f64> = models
            .iter()
            .filter_map(|m| m.estimated_cost_today)
            .collect();
        Ok(UsageView {
            day_start_ms,
            daily_token_budget: self
                .store
                .daily_token_budget()
                .await
                .map_err(to_nexus_error)?,
            tokens_today: used.iter().map(|r| r.prompt_tokens + r.output_tokens).sum(),
            models,
            estimated_cost_today: (!estimates.is_empty()).then(|| estimates.iter().sum()),
        })
    }

    pub async fn set_daily_token_budget(
        &self,
        tokens: Option<u64>,
    ) -> Result<UsageView, NexusError> {
        if let Some(t) = tokens
            && !(1..=MAX_BUDGET).contains(&t)
        {
            return Err(NexusError::new(
                ErrorCode::InvalidRequest,
                format!("a daily budget must be 1..={MAX_BUDGET} tokens"),
            ));
        }
        self.store
            .set_daily_token_budget(tokens, now_ms())
            .await
            .map_err(to_nexus_error)?;
        tracing::info!(?tokens, "daily token budget set");
        self.usage().await
    }

    pub async fn set_model_price(
        &self,
        provider: String,
        model: String,
        price: Option<ModelPrice>,
    ) -> Result<UsageView, NexusError> {
        let invalid = |m: &str| NexusError::new(ErrorCode::InvalidRequest, m);
        if provider.is_empty() || provider.len() > 40 || model.is_empty() || model.len() > 200 {
            return Err(invalid("a provider and a model name are needed"));
        }
        let valid = |x: f64| x.is_finite() && (0.0..=MAX_PRICE).contains(&x);
        if let Some(p) = &price
            && !(valid(p.input_per_million) && valid(p.output_per_million))
        {
            return Err(invalid(&format!(
                "a price must be 0..={MAX_PRICE} US dollars per million tokens"
            )));
        }
        self.store
            .set_model_price(
                provider,
                model,
                price.map(|p| (p.input_per_million, p.output_per_million)),
                now_ms(),
            )
            .await
            .map_err(to_nexus_error)?;
        self.usage().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_day_starts_before_now_and_less_than_a_day_ago() {
        let now = now_ms();
        let start = local_day_start_ms(now);
        assert!(start <= now);
        assert!(now - start < 24 * 60 * 60 * 1000 + 1000);
    }

    #[test]
    fn a_cost_is_tokens_at_the_price_per_million() {
        let price = ModelPrice {
            input_per_million: 1.0,
            output_per_million: 4.0,
        };
        assert!((cost(1_000_000, 500_000, &price) - 3.0).abs() < 1e-9);
        assert_eq!(cost(0, 0, &price), 0.0);
    }
}
