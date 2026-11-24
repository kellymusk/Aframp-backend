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

/// Default page size for `GET /webhooks` when no `limit` is supplied.
pub const DEFAULT_LIST_LIMIT: i64 = 50;
/// Upper bound on `limit` for `GET /webhooks`.
pub const MAX_LIST_LIMIT: i64 = 100;

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
///
/// # DNS rebinding
///
/// A merchant can register a hostname whose A record is public at
/// registration time, then re-point it at an internal address (e.g.
/// `169.254.169.254` or `10.0.0.5`) before delivery — classic DNS rebinding.
/// This resolver re-checks every address it returns against [`is_public_ip`],
/// so a rebound record is rejected at connect time. The remaining risk is a
/// pooled connection that was resolved while the record was still public and
/// is reused for a later delivery without a fresh lookup; [`delivery_client`]
/// disables idle connection reuse so each attempt re-resolves the hostname.
///
/// Mitigations in place:
/// - redirects are not followed ([`reqwest::redirect::Policy::none`]), so a
///   public host can't 30x the request to an internal one;
/// - a 10s request timeout bounds each attempt;
/// - idle connections are not pooled, forcing a fresh DNS lookup per attempt.
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
///
/// Idle connections are not pooled (`pool_max_idle_per_host(0)`) so every
/// delivery attempt performs a fresh DNS lookup through [`PublicOnlyResolver`]
/// instead of reusing a socket resolved before a DNS-rebinding change.
pub fn delivery_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
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

/// Lists a merchant's webhooks with cursor-based pagination.
///
/// Results are ordered by `(created_at, id)` ascending. `cursor` is the
/// opaque `id` of the last item from the previous page; when supplied, only
/// rows strictly after that item are returned. `limit` is clamped to
/// `1..=MAX_LIST_LIMIT`, defaulting to `DEFAULT_LIST_LIMIT`.
pub async fn list(
    db: &PgPool,
    merchant_id: Uuid,
    cursor: Option<Uuid>,
    limit: Option<i64>,
) -> Result<Vec<MerchantWebhook>, sqlx::Error> {
    let limit = limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT);
    sqlx::query_as::<_, MerchantWebhook>(
        "SELECT id, merchant_id, url, created_at FROM merchant_webhooks
          WHERE merchant_id = $1
            AND ($2::uuid IS NULL OR (created_at, id) > (
                SELECT created_at, id FROM merchant_webhooks WHERE id = $2
            ))
          ORDER BY created_at, id
          LIMIT $3",
    )
    .bind(merchant_id)
    .bind(cursor)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Hex HMAC-SHA256 of `body` keyed with `secret` — what receivers recompute
/// to verify [`SIGNATURE_HEADER`].
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length"

/* … truncated 1265 chars — edit only what you need near the top … */
