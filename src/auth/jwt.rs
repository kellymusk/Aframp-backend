use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub merchant_id: Option<Uuid>,
    #[serde(default)]
    pub is_admin: bool,
    pub exp: usize,
    pub iat: usize,
    /// Unique token id, used to revoke a single token on logout. Tokens
    /// issued before this claim existed have none and can't be revoked.
    #[serde(default)]
    pub jti: Option<Uuid>,
}

pub const TOKEN_TTL_HOURS: i64 = 24;

pub fn sign(
    secret: &str,
    user_id: Uuid,
    merchant_id: Option<Uuid>,
    is_admin: bool,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = Utc::now();
    let claims = Claims {
        sub: user_id,
        merchant_id,
        is_admin,
        iat: now.timestamp() as usize,
        exp: (now + Duration::hours(TOKEN_TTL_HOURS)).timestamp() as usize,
        jti: Some(Uuid::new_v4()),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

pub fn verify(secret: &str, token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|d| d.claims)
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error(transparent)]
    Invalid(#[from] jsonwebtoken::errors::Error),
    #[error("token has been revoked")]
    Revoked,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// [`verify`], plus a check that the token's `jti` hasn't been revoked.
pub async fn verify_active(db: &PgPool, secret: &str, token: &str) -> Result<Claims, VerifyError> {
    let claims = verify(secret, token)?;
    if let Some(jti) = claims.jti {
        let revoked: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM revoked_tokens WHERE jti = $1)")
                .bind(jti)
                .fetch_one(db)
                .await?;
        if revoked {
            return Err(VerifyError::Revoked);
        }
    }
    Ok(claims)
}

/// Revokes the token identified by `claims` until it would have expired.
pub async fn revoke(db: &PgPool, claims: &Claims) -> Result<(), sqlx::Error> {
    let Some(jti) = claims.jti else {
        return Ok(());
    };
    let expires_at = chrono::DateTime::<Utc>::from_timestamp(claims.exp as i64, 0)
        .unwrap_or_else(|| Utc::now() + Duration::hours(TOKEN_TTL_HOURS));
    sqlx::query("DELETE FROM revoked_tokens WHERE expires_at < now()")
        .execute(db)
        .await?;
    sqlx::query(
        "INSERT INTO revoked_tokens (jti, expires_at) VALUES ($1, $2)
         ON CONFLICT (jti) DO NOTHING",
    )
    .bind(jti)
    .bind(expires_at)
    .execute(db)
    .await
    .map(|_| ())
}
