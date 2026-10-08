use axum::extract::State;
use axum::Json;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, bad_request_field, conflict, internal, not_found, ApiResult, ErrorCode};
use crate::models::{CreateWalletRequest, Wallet};
use crate::services::wallets::{self, CreateWalletError};
use crate::AppState;

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateWalletRequest>,
) -> ApiResult<Json<Wallet>> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;
    let network = req.network.unwrap_or_else(|| "stellar".into());

    // Issue #948: only networks the deposit worker actually polls.
    if network != "stellar" {
        return Err(bad_request_field(
            "network",
            "unsupported network (only 'stellar' is supported)",
        ));
    }

    let wallet = wallets::create_wallet(&state.db, merchant_id, &network, &state.wallet_encryption_key)
        .await
        .map_err(|err| match err {
            CreateWalletError::AlreadyExists | CreateWalletError::DuplicateNetwork(_) => conflict(
                ErrorCode::InvalidParameters,
                "a wallet already exists for this merchant; use GET /wallet to retrieve it",
            ),
            other => internal(other),
        })?;
    Ok(Json(wallet))
}

pub async fn get(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Wallet>> {
    let merchant_id = auth.merchant_id.ok_or_else(|| {
        bad_request(
            ErrorCode::MerchantNotFound,
            "no merchant associated with this account",
        )
    })?;
    wallets::wallet_by_merchant(&state.db, merchant_id, "stellar")
        .await
        .map_err(internal)?
        .map(Json)
        .ok_or_else(|| not_found(ErrorCode::WalletNotFound, "no wallet created yet"))
}
