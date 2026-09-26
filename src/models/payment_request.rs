use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::validation::{optional_i64, require_i64};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct PaymentRequest {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub wallet_id: Uuid,
    pub amount_stroops: i64,
    pub asset: String,
    pub memo: String,
    pub status: String,
    pub payment_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Parsed create-payment-request body. Built via [`Self::from_json`] so
/// non-integer `amount_stroops` yields a field-level `INVALID_PARAMETERS`
/// error instead of axum's default 422.
#[derive(Debug, Clone)]
pub struct CreatePaymentRequestRequest {
    pub amount_stroops: i64,
    pub asset: Option<String>,
    pub expires_in_secs: Option<i64>,
}

impl CreatePaymentRequestRequest {
    pub fn from_json(body: &Value) -> Result<Self, (&'static str, &'static str)> {
        let amount_stroops =
            require_i64(body, "amount_stroops").map_err(|msg| ("amount_stroops", msg))?;
        let expires_in_secs =
            optional_i64(body, "expires_in_secs").map_err(|msg| ("expires_in_secs", msg))?;
        let asset = match body.get("asset") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(("asset", "must be a string")),
        };
        Ok(Self {
            amount_stroops,
            asset,
            expires_in_secs,
        })
    }
}
