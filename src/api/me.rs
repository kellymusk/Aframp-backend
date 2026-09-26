use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{internal, not_found, ApiResult, ErrorCode};
use crate::services::users;
use crate::AppState;

/// The authenticated merchant's own profile. The JWT only carries ids, so a
/// frontend that reloads with a stored token needs this to render anything
/// human-readable ("signed in as …") without forcing a re-login.
#[derive(Serialize)]
pub struct MeView {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub merchant_id: Option<Uuid>,
    pub merchant_name: Option<String>,
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

pub async fn get(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<MeView>> {
    let user = users::user_by_id(&state.db, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::UserNotFound, "user not found"))?;

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
        is_admin: user.is_admin,
        created_at: user.created_at,
        merchant_id: merchant.as_ref().map(|m| m.id),
        merchant_name: merchant.map(|m| m.name),
        wallet,
        balances,
    }))
}
