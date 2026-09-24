use axum::extract::State;
use axum::Json;

use crate::auth::extractor::AuthUser;
use crate::error::{bad_request, internal, ApiResult, ErrorCode};
use crate::models::{CreateWalletRequest, Wallet};
use crate::services::wallets;
use crate::AppState;

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateWalletRequest>,
) -> ApiResult<Json<Wallet>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    let network = req.network.unwrap_or_else(|| "stellar".into());
    let wallet = wallets::create_wallet(&state.db, merchant_id, &network, &state.wallet_encryption_key)
        .await
        .map_err(internal)?;
    Ok(Json(wallet))
}

pub async fn get(
    State(state): State<AppState>,
    auth: AuthUser,
) -> ApiResult<Json<Wallet>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| bad_request(ErrorCode::MerchantNotFound, "no merchant associated with this account"))?;
    wallets::wallet_by_merchant(&state.db, merchant_id)
        .await
        .map_err(internal)?
        .map(Json)
        .ok_or_else(|| bad_request(ErrorCode::WalletNotFound, "no wallet created yet"))
}