//! DTOs for usage and budgets (`PLAN_NEXUS_RO.md` M2: "bugete de tokeni …;
//! cost estimat separat de utilizarea raportată"). Tokens are what the
//! providers reported for each model call; a cost is only an estimate from
//! prices the user entered, never a bill. Counts stay far below 2^53.

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct UsageView {
    /// Start of today in this PC's local time (unix ms): the day the
    /// budget counts.
    pub day_start_ms: i64,
    /// Tokens (sent and received, every model) allowed per day; `None` is
    /// no limit.
    pub daily_token_budget: Option<u64>,
    pub tokens_today: u64,
    /// Models used today, chosen on a connection, or with a price.
    pub models: Vec<ModelUsageView>,
    /// Sum of the estimates of the models that have one; `None` if none.
    pub estimated_cost_today: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ModelUsageView {
    /// As stored on the connection ("Gemini", "OpenAI", …); empty for
    /// calls recorded before providers were recorded.
    pub provider: String,
    pub model: String,
    pub calls_today: u32,
    pub prompt_tokens_today: u64,
    pub output_tokens_today: u64,
    /// Used through a free tier (no charge expected).
    pub free_tier: bool,
    pub price: Option<ModelPrice>,
    /// Today's tokens at that price; 0 on a free tier.
    pub estimated_cost_today: Option<f64>,
}

/// What the user says a model costs, in US dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
pub struct ModelPrice {
    pub input_per_million: f64,
    pub output_per_million: f64,
}
