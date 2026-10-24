use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{
    bad_gateway, bad_request, bad_request_field, forbidden, internal, ApiResult, ErrorCode,
};
use crate::error::{bad_gateway, bad_request, bad_request_field, internal, ApiResult, ErrorCode};
use crate::models::{CreateWithdrawalRequest, ListParams, NewWithdrawal, Withdrawal};
use serde::{Deserialize, Serialize};
use serde::Deserialize;
use serde_json::Value;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, bad_request_field, internal, ApiResult, ErrorCode};
use crate::models::{CreateWithdrawalRequest, NewWithdrawal, Withdrawal};
use crate::pagination::{Cursor, Page};
use crate::services::withdrawals;
use crate::validation::{is_valid_account_number, is_valid_bank_code};
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

#[derive(serde::Serialize)]
pub struct WithdrawalView {
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
#[derive(Deserialize)]
pub struct VerifyBankParams {
    pub bank_code: String,
    pub account_number: String,
}

#[derive(Serialize)]
pub struct VerifiedAccount {
    pub account_name: String,
    pub bank_code: String,
    pub account_number: String,
}

/// Resolve a bank account (account number + bank code) to its registered
/// account name before any withdrawal is attempted. This lets merchants
/// confirm the recipient details up front instead of discovering a bad
/// account only after a transfer has been attempted.
pub async fn verify_bank(
    State(state): State<AppState>,
    _auth: AuthUser,
    Query(params): Query<VerifyBankParams>,
) -> ApiResult<Json<VerifiedAccount>> {
    if !is_valid_bank_code(&params.bank_code) {
        return Err(bad_request_field("bank_code", "must be a 3-digit code"));
    }
    if !is_valid_account_number(&params.account_number) {
        return Err(bad_request_field(
            "account_number",
            "must be a 10-digit NUBAN account number",
        ));
    }
    let resolved = withdrawals::resolve_account(
        state.payment_provider.as_ref(),
        &params.bank_code,
        &params.account_number,
    )
    .await
    .map_err(map_withdrawal_error)?;
    Ok(Json(VerifiedAccount {
        account_name: resolved.account_name,
        bank_code: params.bank_code,
        account_number: params.account_number,
    }))
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    headers: HeaderMap,
    Json(req): Json<CreateWithdrawalRequest>,
) -> ApiResult<Json<WithdrawalView>> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult<Json<Withdrawal>> {
    let req = CreateWithdrawalRequest::from_json(&body)
        .map_err(|(field, msg)| bad_request_field(field, msg))?;

    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let idempotency_key = match headers.get("Idempotency-Key") {
        Some(value) => {
            let key = value.to_str().map_err(|_| {
                bad_request_field("Idempotency-Key", "must be a valid ASCII string")
            })?;
            if key.is_empty() || key.len() > 255 {
                return Err(bad_request_field(
                    "Idempotency-Key",
                    "must be a non-empty string of at most 255 characters",
                ));
            }
            Some(key.to_string())
        }
        None => None,
    };

    // Refuse if the merchant is suspended.
    let merchant = crate::services::users::merchant_by_id(&state.db, merchant_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "merchant not found"))?;
    if merchant.is_suspended() {
        return Err(crate::error::forbidden(
            ErrorCode::Forbidden,
            "this merchant account has been suspended",
        ));
    }

    if req.amount_stroops <= 0 {
        return Err(bad_request_field(
            "amount_stroops",
            "must be a positive number",
        ));
    }
    if !is_valid_bank_code(&req.bank_code) {
        return Err(bad_request_field("bank_code", "must be a 3-digit code"));
    }
    if !is_valid_account_number(&req.account_number) {
        return Err(bad_request_field(
            "account_number",
            "must be a 10-digit NUBAN account number",
        ));
    }
    let idempotency_key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_owned);
    let withdrawal = withdrawals::create_withdrawal(
        &state.db,
        state.payment_provider.as_ref(),
        NewWithdrawal {
            merchant_id,
            amount_stroops: req.amount_stroops,
            asset: req.asset.unwrap_or_else(|| "cNGN".into()),
            bank_code: req.bank_code,
            account_number: req.account_number,
            idempotency_key,
        },
    )
    .await
    .map_err(map_withdrawal_error)?;
    Ok(Json(to_view(&withdrawal)))
    .await?;
    Ok(Json(withdrawal))
}

pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<WithdrawalView>>> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
) -> ApiResult<Json<Page<Withdrawal>>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let limit = params.merchant_limit();
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(
            Cursor::decode(raw)
                .ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?,
        ),
        None => None,
    };
    let withdrawals =
        withdrawals::withdrawals_by_merchant_cursor(&state.db, merchant_id, limit, cursor)
            .await
            .map_err(internal)?;
    let views: Vec<WithdrawalView> = withdrawals.iter().map(to_view).collect();
    Ok(Json(Page::new(views, limit, |w| Cursor {
        created_at: w.created_at,
        id: w.id,
    })))
}

fn to_view(withdrawal: &Withdrawal) -> WithdrawalView {
    WithdrawalView {
        id: withdrawal.id,
        merchant_id: withdrawal.merchant_id,
        amount_stroops: withdrawal.amount_stroops,
        asset: withdrawal.asset.clone(),
        status: withdrawal.status.clone(),
        provider: withdrawal.provider.clone(),
        provider_reference: withdrawal.provider_reference.clone(),
        bank_code: withdrawal.bank_code.clone(),
        account_number: withdrawal
            .account_number
            .as_deref()
            .map(mask_account_number),
        failure_reason: withdrawal.failure_reason.clone(),
        created_at: withdrawal.created_at,
        updated_at: withdrawal.updated_at,
    }
}

fn mask_account_number(account_number: &str) -> String {
    let last4_start = account_number.len().saturating_sub(4);
    let last4 = &account_number[last4_start..];
    format!("****{last4}")
}

fn map_withdrawal_error(
    err: WithdrawalError,
) -> (axum::http::StatusCode, Json<crate::error::ApiError>) {
    match err {
        WithdrawalError::InsufficientBalance => bad_request(
            ErrorCode::InsufficientBalance,
            "insufficient available balance",
        ),
        WithdrawalError::UnsupportedAsset => bad_request(
            ErrorCode::UnsupportedAsset,
            "withdrawals are only supported for the cNGN asset",
        ),
        WithdrawalError::InvalidAmountPrecision => bad_request(
            ErrorCode::InvalidAmount,
            "amount_stroops must be a whole number of kobo",
        ),
        WithdrawalError::KycRequired => forbidden(
            ErrorCode::KycRequired,
            "merchant KYC verification required before withdrawals",
        ),
        WithdrawalError::AccountResolutionFailed(msg) => {
            bad_request(ErrorCode::AccountResolutionFailed, &msg)
        }
        WithdrawalError::PayoutFailed(msg) => bad_gateway(ErrorCode::PayoutFailed, &msg),
        WithdrawalError::Database(e) => internal(e),
    }
}

#[cfg(test)]
mod pagination_tests {
    use std::collections::HashSet;

    use chrono::{Duration, Utc};
    use uuid::Uuid;

    use crate::pagination::{Cursor, Page};

    /// Minimal row shape mirroring the fields the withdrawals cursor is built from.
    #[derive(Clone)]
    struct Row {
        id: Uuid,
        created_at: chrono::DateTime<Utc>,
    }

    /// In-memory keyset paginator that mirrors the SQL ordering used by
    /// `withdrawals_by_merchant_cursor`: ORDER BY created_at DESC, id DESC.
    fn fetch_page(rows: &[Row], limit: i64, cursor: Option<Cursor>) -> Page<Row> {
        let mut ordered: Vec<Row> = rows.to_vec();
        ordered.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        let filtered: Vec<Row> = ordered
            .into_iter()
            .filter(|r| match &cursor {
                None => true,
                Some(c) => {
                    r.created_at < c.created_at
                        || (r.created_at == c.created_at && r.id < c.id)
                }
            })
            .take(limit as usize)
            .collect();
        Page::new(filtered, limit, |r| Cursor {
            created_at: r.created_at,
            id: r.id,
        })
    }

    fn seed(count: usize) -> Vec<Row> {
        let base = Utc::now();
        (0..count)
            .map(|i| Row {
                id: Uuid::new_v4(),
                created_at: base - Duration::seconds(i as i64),
            })
            .collect()
    }

    #[test]
    fn cursor_pagination_across_multiple_pages() {
        let rows = seed(25);

        // Page 1: 10 items, non-null cursor.
        let page1 = fetch_page(&rows, 10, None);
        assert_eq!(page1.items.len(), 10, "page 1 should have 10 items");
        let cursor1 = page1.next_cursor.clone();
        assert!(cursor1.is_some(), "page 1 should expose a next cursor");

        // Page 2: 10 items, different from page 1, non-null cursor.
        let page2 = fetch_page(&rows, 10, cursor1);
        assert_eq!(page2.items.len(), 10, "page 2 should have 10 items");
        let cursor2 = page2.next_cursor.clone();
        assert!(cursor2.is_some(), "page 2 should expose a next cursor");
        let page1_ids: HashSet<Uuid> = page1.items.iter().map(|r| r.id).collect();
        let page2_ids: HashSet<Uuid> = page2.items.iter().map(|r| r.id).collect();
        assert!(
            page1_ids.is_disjoint(&page2_ids),
            "page 2 must not repeat page 1 rows"
        );

        // Page 3: 5 items, null cursor (last page).
        let page3 = fetch_page(&rows, 10, cursor2);
        assert_eq!(page3.items.len(), 5, "page 3 should have 5 items");
        assert!(page3.next_cursor.is_none(), "last page must have a null cursor");

        // No duplicates or skips across all pages.
        let mut all_ids: Vec<Uuid> = Vec::new();
        all_ids.extend(page1.items.iter().map(|r| r.id));
        all_ids.extend(page2.items.iter().map(|r| r.id));
        all_ids.extend(page3.items.iter().map(|r| r.id));
        let unique: HashSet<Uuid> = all_ids.iter().copied().collect();
        assert_eq!(unique.len(), all_ids.len(), "pagination produced duplicate ids");
        assert_eq!(unique.len(), 25, "pagination skipped rows");
    }

    #[test]
    fn cursor_pagination_stable_with_concurrent_inserts() {
        let mut rows = seed(25);

        // Fetch page 1, then simulate a concurrent insert that sorts before the
        // cursor (newer created_at). It must not shift or duplicate page 2.
        let page1 = fetch_page(&rows, 10, None);
        let cursor1 = page1.next_cursor.clone().expect("page 1 cursor");
        rows.push(Row {
            id: Uuid::new_v4(),
            created_at: Utc::now() + Duration::seconds(1),
        });

        let page2 = fetch_page(&rows, 10, cursor1);
        let page1_ids: HashSet<Uuid> = page1.items.iter().map(|r| r.id).collect();
        let page2_ids: HashSet<Uuid> = page2.items.iter().map(|r| r.id).collect();
        assert_eq!(page2.items.len(), 10, "page 2 should still have 10 items");
        assert!(
            page1_ids.is_disjoint(&page2_ids),
            "concurrent insert must not duplicate rows across pages"
        );
    }
}
