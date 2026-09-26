use sqlx::PgPool;
use uuid::Uuid;

use crate::models::merchant::Merchant;
use crate::models::user::User;

pub async fn user_by_id(db: &PgPool, user_id: Uuid) -> sqlx::Result<Option<User>> {
    sqlx::query_as::<_, User>(
        "SELECT id, email, name, is_admin, created_at FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}

pub async fn merchant_by_user(db: &PgPool, user_id: Uuid) -> sqlx::Result<Option<Merchant>> {
    sqlx::query_as::<_, Merchant>(
        "SELECT id, user_id, name, created_at, updated_at FROM merchants WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
}
