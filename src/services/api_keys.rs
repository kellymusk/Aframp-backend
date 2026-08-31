//! Programmatic API keys: creation, listing, revocation, and the lookup the
//! auth extractor uses to turn a presented key back into a merchant.
//!
//! A key looks like `sk_test_<8-char prefix><32-char secret>`. Only the
//! `sk_<env>_<prefix>` part is stored in the clear; the secret half is hashed
//! with Argon2 exactly like a password, so the database cannot be replayed
//! into working credentials. The prefix exists because Argon2 hashes are
//! salted — without a cheap, indexable handle, authenticating one request
//! would mean verifying it against every key in the table.

use rand::RngCore;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::password;
use crate::models::ApiKey;

/// Characters of the random handle stored in the clear alongside the hash.
const PREFIX_LEN: usize = 8;
/// Characters of the secret half. 32 hex chars = 128 bits.
const SECRET_LEN: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum ApiKeyError {
    #[error("environment must be `test` or `live`")]
    InvalidEnvironment,
    #[error("failed to hash api key")]
    Hashing,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// A newly minted key. `secret` is the only time the full key is ever
/// available — it is not recoverable afterwards, by us or by the merchant.
#[derive(Debug, Clone)]
pub struct CreatedApiKey {
    pub record: ApiKey,
    pub secret: String,
}

/// The merchant (and its owning user) behind a presented key.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ApiKeyPrincipal {
    pub api_key_id: Uuid,
    pub merchant_id: Uuid,
    pub user_id: Uuid,
    pub secret_hash: String,
    pub environment: String,
}

fn random_hex(len: usize) -> String {
    let mut bytes = vec![0u8; len.div_ceil(2)];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)[..len].to_string()
}

/// Splits `sk_test_<prefix><secret>` into the indexable prefix and the secret
/// half. Returns `None` for anything that is not shaped like one of our keys,
/// so a JWT presented on the same header never reaches the database.
pub fn parse_key(presented: &str) -> Option<(String, &str)> {
    let rest = presented.strip_prefix("sk_")?;
    let (environment, handle) = rest.split_once('_')?;
    if environment != "test" && environment != "live" {
        return None;
    }
    if handle.len() != PREFIX_LEN + SECRET_LEN || !handle.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let (prefix, secret) = handle.split_at(PREFIX_LEN);
    Some((format!("sk_{environment}_{prefix}"), secret))
}

#[tracing::instrument(skip_all, err, fields(merchant_id = %merchant_id, %environment))]
pub async fn create(
    db: &PgPool,
    merchant_id: Uuid,
    environment: &str,
) -> Result<CreatedApiKey, ApiKeyError> {
    if environment != "test" && environment != "live" {
        return Err(ApiKeyError::InvalidEnvironment);
    }

    let handle = random_hex(PREFIX_LEN);
    let secret = random_hex(SECRET_LEN);
    let key_prefix = format!("sk_{environment}_{handle}");
    let secret_hash = password::hash(&secret).map_err(|_| ApiKeyError::Hashing)?;

    let record = sqlx::query_as::<_, ApiKey>(
        "INSERT INTO api_keys (merchant_id, key_prefix, secret_hash, environment)
         VALUES ($1, $2, $3, $4)
         RETURNING id, merchant_id, key_prefix, environment, created_at, revoked_at",
    )
    .bind(merchant_id)
    .bind(&key_prefix)
    .bind(&secret_hash)
    .bind(environment)
    .fetch_one(db)
    .await?;

    Ok(CreatedApiKey {
        // The full key, exactly as the caller must send it back.
        secret: format!("{key_prefix}{secret}"),
        record,
    })
}

/// Active keys for a merchant, newest first. Revoked keys are excluded — a
/// revoked key is not a credential, and listing it invites confusion about
/// whether it still works.
#[tracing::instrument(skip_all, err, fields(merchant_id = %merchant_id))]
pub async fn list_active(db: &PgPool, merchant_id: Uuid) -> Result<Vec<ApiKey>, sqlx::Error> {
    sqlx::query_as::<_, ApiKey>(
        "SELECT id, merchant_id, key_prefix, environment, created_at, revoked_at
           FROM api_keys
          WHERE merchant_id = $1 AND revoked_at IS NULL
          ORDER BY created_at DESC",
    )
    .bind(merchant_id)
    .fetch_all(db)
    .await
}

/// Stamps `revoked_at`, scoped to the owning merchant. Returns the revoked
/// row, or `None` when the id does not exist, belongs to someone else, or was
/// already revoked — the three are deliberately indistinguishable to the
/// caller.
#[tracing::instrument(skip_all, err, fields(api_key_id = %id, merchant_id = %merchant_id))]
pub async fn revoke(
    db: &PgPool,
    id: Uuid,
    merchant_id: Uuid,
) -> Result<Option<ApiKey>, sqlx::Error> {
    sqlx::query_as::<_, ApiKey>(
        "UPDATE api_keys
            SET revoked_at = now()
          WHERE id = $1 AND merchant_id = $2 AND revoked_at IS NULL
          RETURNING id, merchant_id, key_prefix, environment, created_at, revoked_at",
    )
    .bind(id)
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

/// Resolves a presented key to its owner, or `None` if it does not
/// authenticate. Verification is Argon2, so a wrong secret costs the same as a
/// right one — the timing of this call says nothing about which half failed.
#[tracing::instrument(skip_all, err)]
pub async fn authenticate(
    db: &PgPool,
    presented: &str,
) -> Result<Option<ApiKeyPrincipal>, sqlx::Error> {
    let Some((key_prefix, secret)) = parse_key(presented) else {
        return Ok(None);
    };

    let candidate = sqlx::query_as::<_, ApiKeyPrincipal>(
        "SELECT k.id AS api_key_id, k.merchant_id, m.user_id, k.secret_hash, k.environment
           FROM api_keys k
           JOIN merchants m ON m.id = k.merchant_id
          WHERE k.key_prefix = $1 AND k.revoked_at IS NULL",
    )
    .bind(&key_prefix)
    .fetch_optional(db)
    .await?;

    let Some(candidate) = candidate else {
        return Ok(None);
    };
    if !password::verify(secret, &candidate.secret_hash) {
        return Ok(None);
    }
    Ok(Some(candidate))
}
