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
    #[error("url must point at a public address, not localhost or a private network")]
    PrivateTarget,
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

/// Whether `ip` is reachable on the public internet. Webhook URLs are
/// merchant-supplied and the backend POSTs to them, so loopback, private,
/// link-local (cloud metadata), CGNAT and similar ranges are refused.
pub fn is_public_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b)) // 100.64.0.0/10 CGNAT
                || (a == 198 && (18..20).contains(&b))) // 198.18.0.0/15 benchmarking
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00 // fc00::/7 unique local
                || (first & 0xffc0) == 0xfe80) // fe80::/10 link-local
        }
    }
}

/// DNS resolver for webhook delivery that only ever returns public addresses,
/// so a hostname can't be pointed (or re-pointed) at an internal service
/// after registration.
pub struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|addr| is_public_ip(addr.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(
                    format!("{} does not resolve to a public address", name.as_str()).into(),
                );
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// HTTP client for delivering merchant webhooks: public addresses only, and
/// redirects are not followed (a redirect could point anywhere).
pub fn delivery_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(std::sync::Arc::new(PublicOnlyResolver))
        .build()
        .expect("failed to build webhook HTTP client")
}

/// Registers `url` for the merchant. Unless `allow_private_targets` (tests,
/// local development), the URL must not name localhost or a non-public IP.
/// Hostnames are checked again at delivery by [`PublicOnlyResolver`].
pub async fn register(
    db: &PgPool,
    merchant_id: Uuid,
    url: &str,
    allow_private_targets: bool,
) -> Result<MerchantWebhook, RegisterWebhookError> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|_| RegisterWebhookError::InvalidUrl)?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
        return Err(RegisterWebhookError::InvalidUrl);
    }
    if !allow_private_targets {
        // `host_str` keeps IPv6 brackets; strip them before parsing.
        let host = parsed
            .host_str()
            .unwrap_or_default()
            .trim_start_matches('[')
            .trim_end_matches(']');
        let private = match host.parse::<std::net::IpAddr>() {
            Ok(ip) => !is_public_ip(ip),
            Err(_) => {
                let d = host.trim_end_matches('.').to_ascii_lowercase();
                d == "localhost"
                    || d.ends_with(".localhost")
                    || d.ends_with(".internal")
                    || d.ends_with(".local")
            }
        };
        if private {
            return Err(RegisterWebhookError::PrivateTarget);
        }
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
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
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
            deliver(
                &db,
                &http,
                &secret,
                delivery_id,
                &webhook.url,
                &event_type,
                &body,
                policy,
            )
            .await;
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

#[cfg(test)]
mod tests {
    use super::is_public_ip;

    #[test]
    fn private_and_internal_addresses_are_not_public() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip} must be refused");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip} is public");
        }
    }
}
