use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: Uuid,
    pub merchant_id: Option<Uuid>,
    pub exp: usize,
    pub iat: usize,
}

pub const TOKEN_TTL_HOURS: i64 = 24;

pub fn sign(secret: &str, user_id: Uuid, merchant_id: Option<Uuid>) -> Result<String, jsonwebtoken::errors::Error> {
    sign_with_ttl(secret, user_id, merchant_id, Duration::hours(TOKEN_TTL_HOURS))
}

/// Like [`sign`] but with an explicit token lifetime. Primarily used by tests
/// that need a token expiring almost immediately (e.g. JWT-expiry enforcement).
pub fn sign_with_ttl(
    secret: &str,
    user_id: Uuid,
    merchant_id: Option<Uuid>,
    ttl: Duration,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = Utc::now();
    let claims = Claims {
        sub: user_id,
        merchant_id,
        iat: now.timestamp() as usize,
        exp: (now + ttl).timestamp() as usize,
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
