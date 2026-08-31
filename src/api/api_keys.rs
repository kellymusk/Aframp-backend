use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::extractor::{AuthMethod, AuthUser};
use crate::error::{bad_request, bad_request_field, forbidden, internal, not_found, ApiResult};
use crate::models::ApiKey;
use crate::services::api_keys::{self, ApiKeyError};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    /// `test` or `live`. Defaults to `test` — the safe half of the choice, so
    /// a client that omits the field cannot accidentally mint a live key.
    pub environment: Option<String>,
}

/// The response to key creation, and the only time the secret is ever
/// transmitted. Everything else in the API refers to a key by `id` or
/// `key_prefix`.
#[derive(Debug, Serialize)]
pub struct CreatedApiKeyView {
    pub id: Uuid,
    pub key_prefix: String,
    pub environment: String,
    pub created_at: DateTime<Utc>,
    /// The full key. Store it now — it is hashed on our side and cannot be
    /// shown again.
    pub secret: String,
}

pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateApiKeyRequest>,
) -> ApiResult<(StatusCode, Json<CreatedApiKeyView>)> {
    // An API key must not be able to mint another API key: otherwise a leaked
    // key survives its own revocation by quietly issuing a replacement.
    if auth.via != AuthMethod::Session {
        return Err(forbidden(
            "api keys must be created with a session; log in with email and password",
        ));
    }
    let merchant_id = merchant_of(&auth)?;
    let environment = req.environment.unwrap_or_else(|| "test".into());

    let created = api_keys::create(&state.db, merchant_id, &environment)
        .await
        .map_err(map_error)?;

    Ok((
        StatusCode::CREATED,
        Json(CreatedApiKeyView {
            id: created.record.id,
            key_prefix: created.record.key_prefix,
            environment: created.record.environment,
            created_at: created.record.created_at,
            secret: created.secret,
        }),
    ))
}

pub async fn list(State(state): State<AppState>, auth: AuthUser) -> ApiResult<Json<Vec<ApiKey>>> {
    let merchant_id = merchant_of(&auth)?;
    let keys = api_keys::list_active(&state.db, merchant_id)
        .await
        .map_err(internal)?;
    Ok(Json(keys))
}

/// Revoking is idempotent from the caller's side only in the sense that a
/// second call 404s: the key is already not a credential, which is what the
/// caller wanted.
pub async fn revoke(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ApiKey>> {
    let merchant_id = merchant_of(&auth)?;
    let revoked = api_keys::revoke(&state.db, id, merchant_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found("api key not found"))?;
    Ok(Json(revoked))
}

fn merchant_of(auth: &AuthUser) -> Result<Uuid, (StatusCode, Json<crate::error::ApiError>)> {
    auth.merchant_id
        .ok_or_else(|| bad_request("no merchant associated with this account"))
}

fn map_error(err: ApiKeyError) -> (StatusCode, Json<crate::error::ApiError>) {
    match err {
        ApiKeyError::InvalidEnvironment => {
            bad_request_field("environment", "must be `test` or `live`")
        }
        other => internal(other),
    }
}
