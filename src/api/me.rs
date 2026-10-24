use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, not_found, ApiResult, ErrorCode};
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{
    bad_request, bad_request_field, conflict, internal, not_found, too_many_requests, ApiResult, ErrorCode,
};
use crate::models::OtpChallengeResponse;
use crate::services::otp::{self, OtpError};
use crate::services::users;
use crate::validation::{normalize_ng_phone_number, validate_name};
use crate::AppState;

/// KYC verification status for a merchant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KycStatus {
    Unverified,
    Pending,
    Verified,
    Rejected,
}

impl KycStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            KycStatus::Unverified => "unverified",
            KycStatus::Pending => "pending",
            KycStatus::Verified => "verified",
            KycStatus::Rejected => "rejected",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "unverified" => Some(KycStatus::Unverified),
            "pending" => Some(KycStatus::Pending),
            "verified" => Some(KycStatus::Verified),
            "rejected" => Some(KycStatus::Rejected),
            _ => None,
        }
    }
}

/// The authenticated merchant's own profile. The JWT only carries ids, so a
/// frontend that reloads with a stored token needs this to render anything
/// human-readable ("signed in as …") without forcing a re-login.
#[derive(Serialize)]
pub struct MeView {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub phone_number: Option<String>,
    pub phone_verified: bool,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub merchant_id: Option<Uuid>,
    pub merchant_name: Option<String>,
    pub merchant_kyc_status: Option<KycStatus>,
    /// Wallet address for the merchant, or `null` if no wallet has been
    /// created yet. Populated in the same round-trip as the profile.
    pub wallet: Option<String>,
    /// Balance summary for the merchant's wallet, or `null` if no wallet
    /// exists yet.
    pub balances: Option<BalanceSummary>,
}

/// Aggregated balance figures for the merchant's wallet.
#[derive(Serialize)]
pub struct BalanceSummary {
    pub available: i64,
    pub pending: i64,
    pub total: i64,
}

/// Row shape for the single JOIN that fetches the merchant's wallet and
/// balance summary alongside the profile lookup.
#[derive(FromRow)]
struct WalletBalanceRow {
    wallet_address: Option<String>,
    available: Option<i64>,
    pending: Option<i64>,
}

/// Body for `PATCH /users/me`. Only the display name is mutable here.
#[derive(Deserialize)]
pub struct UpdateMeRequest {
    pub name: String,
}

pub async fn get(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<MeView>> {
    let row = users::user_with_merchant_by_id(&state.db, auth.user_id)
    let user = users::user_profile_by_id(&state.db, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::UserNotFound, "user not found"))?;

    Ok(Json(MeView {
        user_id: row.user_id,
        email: row.email,
        name: row.name,
        is_admin: row.is_admin,
        created_at: row.created_at,
        merchant_id: row.merchant_id,
        merchant_name: row.merchant_name,
    let merchant = users::merchant_by_user(&state.db, auth.user_id)
        .await
        .map_err(internal)?;

    // Single round-trip: LEFT JOIN so the row is still returned when the
    // merchant has no wallet yet (fields come back as NULL).
    let wallet_row = sqlx::query_as::<_, WalletBalanceRow>(
        r#"
        SELECT
            w.address AS wallet_address,
            b.available AS available,
            b.pending AS pending
        FROM merchants m
        LEFT JOIN wallets w ON w.merchant_id = m.id
        LEFT JOIN wallet_balances b ON b.wallet_id = w.id
        WHERE m.user_id = $1
        "#,
    )
    .bind(auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?;

    let (wallet, balances) = match wallet_row {
        Some(row) => {
            let wallet = row.wallet_address;
            let balances = match (row.available, row.pending) {
                (Some(available), Some(pending)) => Some(BalanceSummary {
                    available,
                    pending,
                    total: available + pending,
                }),
                _ => None,
            };
            (wallet, balances)
        }
        None => (None, None),
    };

    Ok(Json(MeView {
        user_id: user.id,
        email: user.email,
        name: user.name,
        phone_number: user.phone_number,
        phone_verified: user.phone_verified,
        is_admin: user.is_admin,
        created_at: user.created_at,
        merchant_id: merchant.as_ref().map(|m| m.id),
        merchant_name: merchant.as_ref().map(|m| m.name.clone()),
        merchant_kyc_status: merchant
            .as_ref()
            .and_then(|m| KycStatus::from_str(&m.kyc_status)),
    }))
}

/// Request body for a merchant submitting KYC information.
#[derive(Deserialize)]
pub struct SubmitKycRequest {
    pub bvn: Option<String>,
    pub cac_number: Option<String>,
    pub business_name: Option<String>,
}

/// Response returned after a merchant submits KYC information.
#[derive(Serialize)]
pub struct KycView {
    pub merchant_id: Uuid,
    pub merchant_name: String,
    pub kyc_status: KycStatus,
}

/// `PUT /merchants/kyc` — a merchant submits basic KYC info. The merchant's
/// status transitions to `pending` awaiting admin review.
pub async fn submit_kyc(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<SubmitKycRequest>,
) -> ApiResult<Json<KycView>> {
    let merchant = users::merchant_by_user(&state.db, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;

    if req.bvn.is_none() && req.cac_number.is_none() && req.business_name.is_none() {
        return Err(bad_request(
            ErrorCode::InvalidRequest,
            "at least one KYC field (bvn, cac_number, business_name) is required",
        ));
    }

    let updated = users::submit_merchant_kyc(
        &state.db,
        merchant.id,
        req.bvn.as_deref(),
        req.cac_number.as_deref(),
        req.business_name.as_deref(),
    )
    .await
    .map_err(internal)?
    .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;

    let kyc_status = KycStatus::from_str(&updated.kyc_status).unwrap_or(KycStatus::Unverified);

    Ok(Json(KycView {
        merchant_id: updated.id,
        merchant_name: updated.name,
        kyc_status,
    }))
}
        merchant_name: merchant.map(|m| m.name),
        wallet,
        balances,
    }))
}

/// Update the authenticated user's profile name. The merchant row's name
/// mirrors `users.name` (see `users::create_verified`), so both are updated
/// in a single transaction to keep the admin view and merchant profile in
/// sync.
pub async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<UpdateMeRequest>,
) -> ApiResult<Json<MeView>> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(crate::error::bad_request(
            ErrorCode::ValidationError,
            "name must not be empty",
        ));
    }

    users::update_name(&state.db, auth.user_id, name)
        .await
        .map_err(internal)?;

    get(State(state), auth).await
#[derive(Deserialize)]
pub struct UpdateMeRequest {
    pub name: Option<String>,
    pub phone_number: Option<String>,
}

#[derive(Serialize)]
pub struct UpdateMeResponse {
    pub name: String,
    /// The number on file. A requested change only lands here after the
    /// code sent to the new number is verified via `POST /verify-otp`.
    pub phone_number: Option<String>,
    /// Present when a phone change was requested: verify it with
    /// `POST /verify-otp` using this `challenge_id` and the SMS code.
    pub phone_verification: Option<OtpChallengeResponse>,
}

/// Updates the signed-in user's profile. A new name applies immediately; a
/// new phone number is only switched after it's verified by OTP, exactly
/// like signup, so an account can't be pointed at a number its owner
/// doesn't control.
pub async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<UpdateMeRequest>,
) -> ApiResult<Json<UpdateMeResponse>> {
    if req.name.is_none() && req.phone_number.is_none() {
        return Err(bad_request(ErrorCode::InvalidParameters, "provide name and/or phone_number"));
    }
    let name = req
        .name
        .as_deref()
        .map(validate_name)
        .transpose()
        .map_err(|msg| bad_request_field("name", msg))?;
    let new_phone = req
        .phone_number
        .as_deref()
        .map(normalize_ng_phone_number)
        .transpose()
        .map_err(|msg| bad_request_field("phone_number", msg))?;

    let mut user = users::user_by_id(&state.db, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::UserNotFound, "user not found"))?;

    if let Some(name) = name {
        sqlx::query("UPDATE users SET name = $2, updated_at = now() WHERE id = $1")
            .bind(user.id)
            .bind(&name)
            .execute(&state.db)
            .await
            .map_err(internal)?;
        user.name = name;
    }

    let phone_verification = match new_phone {
        Some(phone) if user.phone_number.as_deref() != Some(phone.as_str()) => Some(
            otp::start_phone_change_challenge(
                &state.db,
                state.otp_provider.as_ref(),
                state.otp_hmac_secret.as_str(),
                user.id,
                &phone,
            )
            .await
            .map_err(|err| match err {
                OtpError::PhoneTaken => conflict(ErrorCode::PhoneTaken, "phone number already registered"),
                OtpError::RateLimited => {
                    too_many_requests(ErrorCode::TooManyRequests, "too many requests, please try again shortly")
                }
                other => internal(other),
            })?,
        ),
        _ => None,
    };

    Ok(Json(UpdateMeResponse {
        name: user.name,
        phone_number: user.phone_number,
        phone_verification,
    }))
}

/// Deletes the signed-in account (right to erasure). Personal data is
/// anonymized rather than hard-deleted so the financial records keep a
/// valid owner — see `users::anonymize_and_delete` and the data retention
/// notes in API.md. Every token for the account stops working immediately
/// and the session cookie is cleared.
pub async fn delete(State(state): State<AppState>, auth: AuthUser) -> ApiResult<impl IntoResponse> {
    if !users::anonymize_and_delete(&state.db, auth.user_id).await.map_err(internal)? {
        return Err(not_found(ErrorCode::UserNotFound, "user not found"));
    }
    tracing::info!(user_id = %auth.user_id, "account deleted (personal data anonymized)");
    let cookie = state.cookie.clear().map_err(internal)?;
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]))
}
