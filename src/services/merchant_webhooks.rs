//! Outbound webhooks: merchants register URLs, and events such as
//! `payment.confirmed` are POSTed to each of them, signed with
//! HMAC-SHA256 over the raw body using `WEBHOOK_SECRET`, and retried with
//! exponential backoff. Every delivery is recorded in `webhook_deliveries`.

use std::time::Duration;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::Payment;

/// Header carrying the hex HMAC-SHA256 of the request body.
pub const SIGNATURE_HEADER: &str = "x-aframp-signature";
/// Header carrying the event type, e.g. `payment.confirmed`.
pub const EVENT_HEADER: &str = "x-aframp-event";
pub const PAYMENT_CONFIRMED: &str = "payment.confirmed";

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MerchantWebhook {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub url: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterWebhookError {
    #[error("url must be an absolute http or https URL")]
    InvalidUrl,
    #[error("this webhook URL is already registered")]
    AlreadyRegistered,
    #[error(transparent)]
    Database(sqlx::Error),
}

/// How many times a delivery is attempted and how long to wait between
/// attempts: `base_delay`, then doubling (1s, 2s, 4s, 8s by default).
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_delay: Duration::from_secs(1),
        }
    }
}

pub async fn register(
    db: &PgPool,
    merchant_id: Uuid,
    url: &str,
) -> Result<MerchantWebhook, RegisterWebhookError> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| RegisterWebhookError::InvalidUrl)?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
        return Err(RegisterWebhookError::InvalidUrl);
    }

    sqlx::query_as::<_, MerchantWebhook>(
        "INSERT INTO merchant_webhooks (merchant_id, url) VALUES ($1, $2)
         RETURNING id, merchant_id, url, created_at",
    )
    .bind(merchant_id)
    .bind(parsed.as_str())
    .fetch_one(db)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
            RegisterWebhookError::AlreadyRegistered
        }
        _ => RegisterWebhookError::Database(e),
    })
}

pub async fn list(db: &PgPool, merchant_id: Uuid) -> Result<Vec<MerchantWebhook>, sqlx::Error> {
    sqlx::query_as::<_, MerchantWebhook>(
        "SELECT id, merchant_id, url, created_at FROM merchant_webhooks
          WHERE merchant_id = $1 ORDER BY created_at",
    )
    .bind(merchant_id)
    .fetch_all(db)
    .await
}

/// Hex HMAC-SHA256 of `body` keyed with `secret` — what receivers recompute
/// to verify [`SIGNATURE_HEADER`].
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// The `payment.confirmed` event body.
pub fn payment_confirmed_payload(payment: &Payment) -> serde_json::Value {
    serde_json::json!({
        "type": PAYMENT_CONFIRMED,
        "created_at": Utc::now(),
        "data": {
            "payment_id": payment.id,
            "merchant_id": payment.merchant_id,
            "wallet_address": payment.wallet_address,
            "tx_hash": payment.tx_hash,
            "amount_stroops": payment.amount_stroops,
            "asset": payment.asset,
            "network": payment.network,
        },
    })
}

/// Record a delivery for every webhook the merchant registered and deliver
/// each one in the background. Returns the delivery ids.
pub async fn dispatch(
    db: &PgPool,
    http: &reqwest::Client,
    secret: &str,
    merchant_id: Uuid,
    event_type: &str,
    payload: &serde_json::Value,
    policy: RetryPolicy,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let body = payload.to_string();
    let mut ids = Vec::new();
    for webhook in list(db, merchant_id).await? {
        let delivery_id: Uuid = sqlx::query_scalar(
            "INSERT INTO webhook_deliveries (webhook_id, event_type, payload)
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(webhook.id)
        .bind(event_type)
        .bind(&body)
        .fetch_one(db)
        .await?;
        ids.push(delivery_id);

        let (db, http, secret, event_type, body) = (
            db.clone(),
            http.clone(),
            secret.to_string(),
            event_type.to_string(),
            body.clone(),
        );
        tokio::spawn(async move {
            deliver(&db, &http, &secret, delivery_id, &webhook.url, &event_type, &body, policy).await;
        });
    }
    Ok(ids)
}

/// POST `body` to `url` until it gets a 2xx or `policy.max_attempts` is
/// reached, updating the delivery row after every attempt.
#[allow(clippy::too_many_arguments)]
pub async fn deliver(
    db: &PgPool,
    http: &reqwest::Client,
    secret: &str,
    delivery_id: Uuid,
    url: &str,
    event_type: &str,
    body: &str,
    policy: RetryPolicy,
) {
    let signature = sign(secret, body.as_bytes());
    let mut delay = policy.base_delay;

    for attempt in 1..=policy.max_attempts {
        let result = http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(SIGNATURE_HEADER, &signature)
            .header(EVENT_HEADER, event_type)
            .body(body.to_string())
            .send()
            .await;
        let error = match result {
            Ok(resp) if resp.status().is_success() => None,
            Ok(resp) => Some(format!("HTTP {}", resp.status())),
            Err(e) => Some(e.to_string()),
        };

        let status = match (&error, attempt == policy.max_attempts) {
            (None, _) => "delivered",
            (Some(_), true) => "failed",
            (Some(_), false) => "pending",
        };
        if let Err(e) = sqlx::query(
            "UPDATE webhook_deliveries
                SET status = $2, attempts = $3, last_error = $4, updated_at = now()
              WHERE id = $1",
        )
        .bind(delivery_id)
        .bind(status)
        .bind(attempt as i32)
        .bind(&error)
        .execute(db)
        .await
        {
            tracing::warn!(error = %e, %delivery_id, "failed to record webhook delivery attempt");
        }

        match error {
            None => {
                tracing::info!(%delivery_id, url, event_type, attempt, "webhook delivered");
                return;
            }
            Some(err) => {
                tracing::warn!(%delivery_id, url, event_type, attempt, error = %err, "webhook delivery attempt failed");
                if attempt < policy.max_attempts {
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
            }
        }
    }
}
