use axum::extract::State;
use axum::Json;
use chrono::{Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{
    bad_request, bad_request_field, conflict, internal, not_found, ApiResult, ErrorCode,
};
use crate::models::Merchant;
use crate::services::users;
use crate::validation::{is_valid_email, validate_name};
use crate::AppState;

/// How long an email-change confirmation token stays valid.
const EMAIL_CHANGE_TTL_MINS: i64 = 30;

/// `GET /merchants/me` — the signed-in user's merchant profile on its own,
/// separate from the auth-identity `GET /me`, so richer business fields can be
/// surfaced here without bloating the identity endpoint.
pub async fn get(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Merchant>> {
    let merchant = users::merchant_by_user(&state.db, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;
    Ok(Json(merchant))
}

#[derive(Deserialize)]
pub struct UpdateMerchantRequest {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Serialize)]
pub struct UpdateMerchantResponse {
    pub merchant_id: Uuid,
    pub name: String,
    /// The account's email. A requested change only lands here after it is
    /// confirmed via `POST /merchants/me/email/confirm`.
    pub email: String,
    /// Set while an email change is awaiting confirmation.
    pub pending_email: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ProfileRow {
    id: Uuid,
    name: String,
    email: String,
    pending_email: Option<String>,
}

fn merchant_id(auth: &AuthUser) -> ApiResult<Uuid> {
    auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })
}

fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// `PATCH /merchants/me` — update the business name and/or the account email.
/// A name change applies immediately; an email change is held as pending and
/// a confirmation token is sent to the new address.
pub async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<UpdateMerchantRequest>,
) -> ApiResult<Json<UpdateMerchantResponse>> {
    let merchant_id = merchant_id(&auth)?;
    if req.name.is_none() && req.email.is_none() {
        return Err(bad_request(
            ErrorCode::InvalidParameters,
            "provide name and/or email",
        ));
    }
    let name = req
        .name
        .as_deref()
        .map(validate_name)
        .transpose()
        .map_err(|msg| bad_request_field("name", msg))?;
    let email = match req.email.as_deref().map(str::trim) {
        Some(e) if !is_valid_email(e) => {
            return Err(bad_request_field("email", "must be a valid email address"))
        }
        other => other.map(str::to_lowercase),
    };

    if let Some(name) = &name {
        sqlx::query("UPDATE merchants SET name = $2, updated_at = now() WHERE id = $1")
            .bind(merchant_id)
            .bind(name)
            .execute(&state.db)
            .await
            .map_err(internal)?;
    }

    if let Some(email) = &email {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token = hex::encode(bytes);
        sqlx::query(
            "UPDATE users
                SET pending_email = $2, email_change_token_hash = $3, email_change_expires_at = $4,
                    updated_at = now()
              WHERE id = $1",
        )
        .bind(auth.user_id)
        .bind(email)
        .bind(hash_token(&token))
        .bind(Utc::now() + Duration::minutes(EMAIL_CHANGE_TTL_MINS))
        .execute(&state.db)
        .await
        .map_err(internal)?;
        send_email_confirmation(email, &token);
    }

    profile(&state, merchant_id).await.map(Json)
}

#[derive(Deserialize)]
pub struct ConfirmEmailRequest {
    pub token: String,
}

/// `POST /merchants/me/email/confirm` — swap in the pending email once the
/// token sent to it is presented. Receiving the token proves the inbox, so
/// the new address also counts as verified.
pub async fn confirm_email(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<ConfirmEmailRequest>,
) -> ApiResult<Json<UpdateMerchantResponse>> {
    let merchant_id = merchant_id(&auth)?;
    let updated = sqlx::query(
        "UPDATE users
            SET email = pending_email, email_verified = true, pending_email = NULL,
                email_change_token_hash = NULL, email_change_expires_at = NULL, updated_at = now()
          WHERE id = $1 AND pending_email IS NOT NULL
            AND email_change_token_hash = $2 AND email_change_expires_at > now()",
    )
    .bind(auth.user_id)
    .bind(hash_token(req.token.trim()))
    .execute(&state.db)
    .await
    .map_err(
        |err| match err.as_database_error().map(|e| e.is_unique_violation()) {
            Some(true) => conflict(ErrorCode::EmailTaken, "email already registered"),
            _ => internal(err),
        },
    )?;
    if updated.rows_affected() == 0 {
        return Err(bad_request(
            ErrorCode::InvalidParameters,
            "confirmation token is invalid or expired",
        ));
    }

    profile(&state, merchant_id).await.map(Json)
}

async fn profile(state: &AppState, merchant_id: Uuid) -> ApiResult<UpdateMerchantResponse> {
    let row = sqlx::query_as::<_, ProfileRow>(
        "SELECT m.id, m.name, u.email, u.pending_email
           FROM merchants m JOIN users u ON u.id = m.user_id
          WHERE m.id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(&state.db)
    .await
    .map_err(internal)?
    .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;
    Ok(UpdateMerchantResponse {
        merchant_id: row.id,
        name: row.name,
        email: row.email,
        pending_email: row.pending_email,
    })
}

/// Delivers the email-change confirmation token to the new address. No
/// transactional email provider is wired in yet, so this only records that a
/// token was issued; the token itself is a credential and is never logged.
fn send_email_confirmation(email: &str, _token: &str) {
    tracing::info!(
        email,
        "email change confirmation issued (no email provider configured)"
    );
}
