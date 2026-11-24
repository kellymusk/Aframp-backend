use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{
    error::AppError,
    middleware::auth::AuthUser,
    services::paystack::PaystackService,
    state::AppState,
};

#[derive(Debug, Deserialize)]
pub struct ListBanksQuery {
    pub country: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Bank {
    pub id: i64,
    pub name: String,
    pub code: String,
    pub slug: Option<String>,
    pub country: Option<String>,
    pub currency: Option<String>,
    pub r#type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BanksResponse {
    pub banks: Vec<Bank>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveAccountQuery {
    pub bank_code: String,
    pub account_number: String,
}

#[derive(Debug, Serialize)]
pub struct ResolveAccountResponse {
    pub account_name: String,
    pub account_number: String,
    pub bank_code: String,
}

/// GET /banks
pub async fn list_banks(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListBanksQuery>,
) -> Result<impl IntoResponse, AppError> {
    let country = query.country.unwrap_or_else(|| "nigeria".to_string());
    let banks = state.paystack.list_banks(&country).await?;

    let banks = banks
        .into_iter()
        .map(|b| Bank {
            id: b.id,
            name: b.name,
            code: b.code,
            slug: b.slug,
            country: b.country,
            currency: b.currency,
            r#type: b.bank_type,
        })
        .collect();

    Ok(Json(BanksResponse { banks }))
}

/// GET /banks/resolve?bank_code=&account_number=
///
/// Resolves the account holder's name for a given bank code and account
/// number. Requires authentication and is rate-limited per merchant.
/// Fails clearly (no placeholder name) when resolution is unsuccessful.
pub async fn resolve_account(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(query): Query<ResolveAccountQuery>,
) -> Result<impl IntoResponse, AppError> {
    let bank_code = query.bank_code.trim();
    let account_number = query.account_number.trim();

    if bank_code.is_empty() {
        return Err(AppError::BadRequest("bank_code is required".to_string()));
    }
    if account_number.is_empty() {
        return Err(AppError::BadRequest(
            "account_number is required".to_string(),
        ));
    }

    // Rate-limit per merchant to protect the upstream provider.
    state
        .rate_limiter
        .check_account_resolution(auth.user_id)
        .await?;

    let resolved = state
        .paystack
        .resolve_account_number(account_number, bank_code)
        .await?;

    let account_name = resolved.account_name.trim().to_string();
    if account_name.is_empty() {
        return Err(AppError::Upstream(
            "Could not resolve account name for the provided details".to_string(),
        ));
    }

    Ok(Json(ResolveAccountResponse {
        account_name,
        account_number: account_number.to_string(),
        bank_code: bank_code.to_string(),
    }))
}
