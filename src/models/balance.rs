use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Balance {
    pub merchant_id: Uuid,
    pub asset: String,
    pub available: i64,
    pub pending: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpdateBalance {
    pub merchant_id: Uuid,
    pub asset: String,
    pub available_delta: i64,
    pub pending_delta: i64,
}

impl UpdateBalance {
    /// Build the single UPSERT used to apply a confirmed deposit directly to
    /// the merchant's balance.
    ///
    /// A confirmed deposit credits `available` by `amount` in one statement,
    /// replacing the previous two `apply_delta` round-trips (pending credit,
    /// then pending -> available move).
    ///
    /// TODO(confirmation-depth): once confirmation depth is implemented, the
    /// pending credit and the pending -> available move must be separated again
    /// (credit `pending` on broadcast, then move to `available` on finality)
    /// instead of applying the net delta in a single UPSERT.
    pub fn confirmed_deposit(merchant_id: Uuid, asset: impl Into<String>, amount: i64) -> Self {
        Self {
            merchant_id,
            asset: asset.into(),
            available_delta: amount,
            pending_delta: 0,
        }
    }
}
