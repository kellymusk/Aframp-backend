use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::validation::require_i64;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Withdrawal {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub amount_stroops: i64,
    pub asset: String,
    pub status: String,
    pub provider: Option<String>,
    pub provider_reference: Option<String>,
    pub bank_code: Option<String>,
    pub account_number: Option<String>,
    pub failure_reason: Option<String>,
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Parsed withdraw body. Built via [`Self::from_json`] so non-integer
/// `amount_stroops` yields a field-level `INVALID_PARAMETERS` error.
#[derive(Debug, Clone)]
pub struct CreateWithdrawalRequest {
    pub amount_stroops: i64,
    pub asset: Option<String>,
    pub bank_code: String,
    pub account_number: String,
}

impl CreateWithdrawalRequest {
    pub fn from_json(body: &Value) -> Result<Self, (&'static str, &'static str)> {
        let amount_stroops =
            require_i64(body, "amount_stroops").map_err(|msg| ("amount_stroops", msg))?;
        let asset = match body.get("asset") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(("asset", "must be a string")),
        };
        let bank_code = match body.get("bank_code") {
            Some(Value::String(s)) => s.clone(),
            Some(_) => return Err(("bank_code", "must be a string")),
            None => return Err(("bank_code", "is required")),
        };
        let account_number = match body.get("account_number") {
            Some(Value::String(s)) => s.clone(),
            Some(_) => return Err(("account_number", "must be a string")),
            None => return Err(("account_number", "is required")),
        };
        Ok(Self {
            amount_stroops,
            asset,
            bank_code,
            account_number,
        })
    }
}

#[derive(Debug, Clone)]
pub struct NewWithdrawal {
    pub merchant_id: Uuid,
    pub amount_stroops: i64,
    pub asset: String,
    pub bank_code: String,
    pub account_number: String,
    pub idempotency_key: Option<String>,
}
