use chrono::{Duration, Utc};
use rand::RngCore;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::PaymentRequest;

const DEFAULT_EXPIRY_SECS: i64 = 15 * 60;
const MIN_EXPIRY_SECS: i64 = 60;
const MAX_EXPIRY_SECS: i64 = 24 * 60 * 60;

/// Default edge length (in pixels) of a rendered payment request QR code.
pub const DEFAULT_QR_SIZE: u32 = 256;
/// Smallest QR image we will render, to keep codes scannable.
pub const MIN_QR_SIZE: u32 = 64;
/// Largest QR image we will render, to bound memory/CPU per request.
pub const MAX_QR_SIZE: u32 = 1024;

#[derive(Debug, thiserror::Error)]
pub enum PaymentRequestError {
    #[error("amount_stroops must be positive")]
    InvalidAmount,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum QrError {
    #[error("payment request not found")]
    NotFound,
    #[error("failed to encode QR code: {0}")]
    Encode(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

fn generate_memo() -> String {
    let mut bytes = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub async fn create_payment_request(
    db: &PgPool,
    merchant_id: Uuid,
    wallet_id: Uuid,
    amount_stroops: i64,
    asset: String,
    expires_in_secs: Option<i64>,
) -> Result<PaymentRequest, PaymentRequestError> {
    if amount_stroops <= 0 {
        return Err(PaymentRequestError::InvalidAmount);
    }
    let ttl = expires_in_secs
        .unwrap_or(DEFAULT_EXPIRY_SECS)
        .clamp(MIN_EXPIRY_SECS, MAX_EXPIRY_SECS);
    let expires_at = Utc::now() + Duration::seconds(ttl);
    let memo = generate_memo();

    sqlx::query_as::<_, PaymentRequest>(
        "INSERT INTO payment_requests (merchant_id, wallet_id, amount_stroops, asset, memo, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                   expires_at, created_at, updated_at",
    )
    .bind(merchant_id)
    .bind(wallet_id)
    .bind(amount_stroops)
    .bind(&asset)
    .bind(&memo)
    .bind(expires_at)
    .fetch_one(db)
    .await
    .map_err(PaymentRequestError::from)
}

/// A payment request joined with its wallet's address, so listing many doesn't
/// fan out into one wallet lookup per row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PaymentRequestWithWallet {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub wallet_id: Uuid,
    pub amount_stroops: i64,
    pub asset: String,
    pub memo: String,
    pub status: String,
    pub payment_id: Option<Uuid>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub address: String,
    pub network: String,
}

pub async fn payment_requests_by_merchant(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
) -> Result<Vec<PaymentRequestWithWallet>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequestWithWallet>(
        "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                pr.status, pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at,
                w.address, w.network
           FROM payment_requests pr
           JOIN wallets w ON w.id = pr.wallet_id
          WHERE pr.merchant_id = $1
          ORDER BY pr.created_at DESC
          LIMIT $2",
    )
    .bind(merchant_id)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Keyset-paginated variant of [`payment_requests_by_merchant`]. Orders by
/// `(created_at, id)` DESC so concurrent inserts can't shift rows across
/// pages the way an OFFSET-based scan can.
pub async fn payment_requests_by_merchant_cursor(
    db: &PgPool,
    merchant_id: Uuid,
    limit: i64,
    cursor: Option<crate::pagination::Cursor>,
) -> Result<Vec<PaymentRequestWithWallet>, sqlx::Error> {
    match cursor {
        Some(c) => {
            sqlx::query_as::<_, PaymentRequestWithWallet>(
                "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                        pr.status, pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at,
                        w.address, w.network
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1
                    AND (pr.created_at, pr.id) < ($2, $3)
                  ORDER BY pr.created_at DESC, pr.id DESC
                  LIMIT $4",
            )
            .bind(merchant_id)
            .bind(c.created_at)
            .bind(c.id)
            .bind(limit + 1)
            .fetch_all(db)
            .await
        }
        None => {
            sqlx::query_as::<_, PaymentRequestWithWallet>(
                "SELECT pr.id, pr.merchant_id, pr.wallet_id, pr.amount_stroops, pr.asset, pr.memo,
                        pr.status, pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at,
                        w.address, w.network
                   FROM payment_requests pr
                   JOIN wallets w ON w.id = pr.wallet_id
                  WHERE pr.merchant_id = $1
                  ORDER BY pr.created_at DESC, pr.id DESC
                  LIMIT $2",
            )
            .bind(merchant_id)
            .bind(limit + 1)
            .fetch_all(db)
            .await
        }
    }
}

pub async fn payment_request_by_id(db: &PgPool, id: Uuid) -> Result<Option<PaymentRequest>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequest>(
        "SELECT id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                expires_at, created_at, updated_at
           FROM payment_requests WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// Looks up the pending request a detected deposit's memo correlates to, if any.
pub async fn find_pending_by_wallet_and_memo(
    db: &PgPool,
    wallet_id: Uuid,
    memo: &str,
) -> Result<Option<PaymentRequest>, sqlx::Error> {
    sqlx::query_as::<_, PaymentRequest>(
        "SELECT id, merchant_id, wallet_id, amount_stroops, asset, memo, status, payment_id,
                expires_at, created_at, updated_at
           FROM payment_requests
          WHERE wallet_id = $1 AND memo = $2 AND status = 'pending'",
    )
    .bind(wallet_id)
    .bind(memo)
    .fetch_optional(db)
    .await
}

pub async fn mark_paid(db: &PgPool, id: Uuid, payment_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE payment_requests SET status = 'paid', payment_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(payment_id)
    .execute(db)
    .await
    .map(|_| ())
}

pub async fn mark_partial(db: &PgPool, id: Uuid, payment_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE payment_requests SET status = 'partial', payment_id = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(payment_id)
    .execute(db)
    .await
    .map(|_| ())
}

/// Clamps a caller-supplied `size` query parameter into the supported range,
/// falling back to [`DEFAULT_QR_SIZE`] when absent.
pub fn normalize_qr_size(size: Option<u32>) -> u32 {
    size.unwrap_or(DEFAULT_QR_SIZE).clamp(MIN_QR_SIZE, MAX_QR_SIZE)
}

/// Renders `sep7_uri` as a PNG QR code with an edge length of `size` pixels.
///
/// The QR module grid is scaled up by an integer factor so the resulting image
/// stays crisp, then padded with a quiet zone and centered on a white canvas of
/// exactly `size` x `size` pixels.
pub fn render_qr_png(sep7_uri: &str, size: u32) -> Result<Vec<u8>, QrError> {
    use image::{ImageBuffer, Luma};
    use qrcode::QrCode;

    let size = normalize_qr_size(Some(size));
    let code = QrCode::new(sep7_uri.as_bytes()).map_err(|e| QrError::Encode(e.to_string()))?;
    let modules = code.to_colors();
    let width = code.width() as u32;

    // Quiet zone (4 modules) required by the QR spec for reliable scanning.
    let quiet = 4u32;
    let total_modules = width + quiet * 2;
    let scale = (size / total_modules).max(1);
    let rendered = total_modules * scale;
    let offset = (size.saturating_sub(rendered)) / 2;

    let mut img: ImageBuffer<Luma<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(size, size, Luma([255u8]));

    for y in 0..width {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            if modules[idx] == qrcode::Color::Dark {
                let px0 = offset + (x + quiet) * scale;
                let py0 = offset + (y + quiet) * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = px0 + dx;
                        let py = py0 + dy;
                        if px < size && py < size {
                            img.put_pixel(px, py, Luma([0u8]));
                        }
                    }
                }
            }
        }
    }

    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| QrError::Encode(e.to_string()))?;
    Ok(png)
}

/// Builds the SEP-7 URI a wallet should scan to pay this request.
pub fn sep7_uri_for(request: &PaymentRequest, address: &str, network: &str) -> String {
    let amount = request.amount_stroops as f64 / 10_000_000f64;
    format!(
        "web+stellar:pay?destination={}&amount={}&asset_code={}&memo={}&memo_type=text&network={}",
        address, amount, request.asset, request.memo, network
    )
}

/// Renders the QR code image for a payment request, caching the result by
/// `(id, size)` so repeated POS polls don't re-encode the same PNG.
pub async fn payment_request_qr_png(
    db: &PgPool,
    cache: &crate::services::qr_cache::QrCache,
    id: Uuid,
    size: Option<u32>,
) -> Result<Vec<u8>, QrError> {
    let size = normalize_qr_size(size);
    if let Some(png) = cache.get(id, size) {
        return Ok(png);
    }

    let request = payment_request_by_id(db, id)
        .await?
        .ok_or(QrError::NotFound)?;

    let wallet = sqlx::query_as::<_, (String, String)>(
        "SELECT address, network FROM wallets WHERE id = $1",
    )
    .bind(request.wallet_id)
    .fetch_optional(db)
    .await?
    .ok_or(QrError::NotFound)?;

    let uri = sep7_uri_for(&request, &wallet.0, &wallet.1);
    let png = render_qr_png(&uri, size)?;
    cache.insert(id, size, png.clone());
    Ok(png)
}
