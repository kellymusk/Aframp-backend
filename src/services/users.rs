use sqlx::PgPool;

use crate::auth::password;
use crate::models::{Merchant, User};

#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("email already registered")]
    EmailTaken,
    #[error("invalid email or password")]
    InvalidCredentials,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("password hashing failed")]
    Hash,
}

pub async fn signup(
    db: &PgPool,
    email: &str,
    password_raw: &str,
    name: &str,
) -> Result<(User, Merchant), UserError> {
    let password_hash = password::hash(password_raw).map_err(|_| UserError::Hash)?;

    let mut tx = db.begin().await?;
    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (email, password_hash, name)
         VALUES ($1, $2, $3)
         RETURNING id, email, password_hash, name, created_at, updated_at",
    )
    .bind(email)
    .bind(&password_hash)
    .bind(name)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_insert_error)?;

    let merchant = sqlx::query_as::<_, Merchant>(
        "INSERT INTO merchants (user_id, name)
         VALUES ($1, $2)
         RETURNING id, user_id, name, created_at",
    )
    .bind(user.id)
    .bind(name)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok((user, merchant))
}

fn map_insert_error(err: sqlx::Error) -> UserError {
    if is_unique_violation(&err) {
        UserError::EmailTaken
    } else {
        UserError::Database(err)
    }
}

fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(
        err,
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505")
    )
}

pub async fn login(db: &PgPool, email: &str, password_raw: &str) -> Result<(User, Option<Merchant>), UserError> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, name, created_at, updated_at FROM users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(db)
    .await?
    .ok_or(UserError::InvalidCredentials)?;

    if !password::verify(password_raw, &user.password_hash) {
        return Err(UserError::InvalidCredentials);
    }

    let merchant = sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user.id)
    .fetch_optional(db)
    .await?;

    Ok((user, merchant))
}

pub async fn user_by_id(db: &PgPool, user_id: uuid::Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash, name, created_at, updated_at FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_user(db: &PgPool, user_id: uuid::Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE user_id = $1 LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_id(db: &PgPool, merchant_id: uuid::Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at FROM merchants WHERE id = $1",
    )
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}
