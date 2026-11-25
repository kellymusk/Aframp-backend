use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    auth::MerchantAuth,
    error::{ApiError, ApiResult},
    AppState,
};

#[derive(Debug, Serialize, FromRow)]
pub struct Contact {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub name: String,
    pub address: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateContact {
    pub name: String,
    pub address: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateContact {
    pub name: Option<String>,
    pub address: Option<String>,
}

/// Validate a Stellar public key: 56 chars, starts with `G`, base32 alphabet.
fn validate_stellar_address(address: &str) -> ApiResult<()> {
    let valid = address.len() == 56
        && address.starts_with('G')
        && address
            .chars()
            .all(|c| matches!(c, 'A'..='Z' | '2'..='7'));
    if valid {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "address must be a valid Stellar G-address".to_string(),
        ))
    }
}

fn validate_name(name: &str) -> ApiResult<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 255 {
        return Err(ApiError::BadRequest(
            "name must be between 1 and 255 characters".to_string(),
        ));
    }
    Ok(())
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/contacts", get(list_contacts).post(create_contact))
        .route(
            "/contacts/{id}",
            patch(update_contact).delete(delete_contact),
        )
}

async fn list_contacts(
    State(state): State<AppState>,
    auth: MerchantAuth,
) -> ApiResult<impl IntoResponse> {
    let contacts = sqlx::query_as::<_, Contact>(
        "SELECT id, merchant_id, name, address, created_at, updated_at \
         FROM contacts WHERE merchant_id = $1 ORDER BY created_at DESC",
    )
    .bind(auth.merchant_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(contacts))
}

async fn create_contact(
    State(state): State<AppState>,
    auth: MerchantAuth,
    Json(payload): Json<CreateContact>,
) -> ApiResult<impl IntoResponse> {
    validate_name(&payload.name)?;
    validate_stellar_address(&payload.address)?;

    let contact = sqlx::query_as::<_, Contact>(
        "INSERT INTO contacts (merchant_id, name, address) \
         VALUES ($1, $2, $3) \
         RETURNING id, merchant_id, name, address, created_at, updated_at",
    )
    .bind(auth.merchant_id)
    .bind(payload.name.trim())
    .bind(&payload.address)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(contact)))
}

async fn update_contact(
    State(state): State<AppState>,
    auth: MerchantAuth,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateContact>,
) -> ApiResult<impl IntoResponse> {
    if let Some(name) = &payload.name {
        validate_name(name)?;
    }
    if let Some(address) = &payload.address {
        validate_stellar_address(address)?;
    }

    let contact = sqlx::query_as::<_, Contact>(
        "UPDATE contacts SET \
            name = COALESCE($3, name), \
            address = COALESCE($4, address), \
            updated_at = NOW() \
         WHERE id = $1 AND merchant_id = $2 \
         RETURNING id, merchant_id, name, address, created_at, updated_at",
    )
    .bind(id)
    .bind(auth.merchant_id)
    .bind(payload.name.as_deref().map(str::trim))
    .bind(payload.address.as_deref())
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("contact not found".to_string()))?;

    Ok(Json(contact))
}

async fn delete_contact(
    State(state): State<AppState>,
    auth: MerchantAuth,
    Path(id): Path<Uuid>,
) -> ApiResult<impl IntoResponse> {
    let result = sqlx::query("DELETE FROM contacts WHERE id = $1 AND merchant_id = $2")
        .bind(id)
        .bind(auth.merchant_id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("contact not found".to_string()));
    }

    Ok(StatusCode::NO_CONTENT)
}
