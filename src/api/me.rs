use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::auth::extractor::AuthUser;
use crate::error::{internal, not_found, ApiResult};
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
    pub created_at: DateTime<Utc>,
    pub merchant_id: Option<Uuid>,
    pub merchant_name: Option<String>,
}

pub async fn get(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<MeView>> {
    let user = users::user_by_id(&state.db, auth.user_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found("user not found"))?;

    let merchant = users::merchant_by_user(&state.db, auth.user_id)
        .await
        .map_err(internal)?;

    Ok(Json(MeView {
        user_id: user.id,
        email: user.email,
        name: user.name,
        created_at: user.created_at,
        merchant_id: merchant.as_ref().map(|m| m.id),
        merchant_name: merchant.map(|m| m.name),
    }))
}
