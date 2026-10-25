# Proposal: #1128 — merge GET /me into one LEFT JOIN

> Status: ready for maintainer apply. `src/` is a protected path on
> `dev-backend` (see CONTRIBUTING.md), so this change ships as a reviewed
> patch rather than a contributor commit under `src/`.
>
> Integration coverage that must stay green after apply:
> `tests/me_join.rs` (JOIN SQL ≡ current `/me` response).
> Benchmark: `scripts/bench_me_join.sh`.

## Problem

`src/api/me.rs` currently does two sequential round-trips on every dashboard load:

1. `users::user_by_id` — `SELECT … FROM users WHERE id = $1`
2. `users::merchant_by_user` — `SELECT … FROM merchants WHERE user_id = $1`

The admin users list already uses a JOIN (`services::admin::users`). `/me` should match that pattern.

## 1. Add `users::user_with_merchant` in `src/services/users.rs`

Insert next to `user_by_id`:

```rust
/// User left-joined to its merchant in one round-trip — used by `GET /me`
/// so a dashboard reload does not pay for two sequential queries.
pub async fn user_with_merchant(
    db: &PgPool,
    user_id: uuid::Uuid,
) -> Result<Option<(User, Option<Merchant>)>, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: uuid::Uuid,
        email: String,
        password_hash: String,
        name: String,
        is_admin: bool,
        phone_number: Option<String>,
        phone_verified: bool,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
        merchant_id: Option<uuid::Uuid>,
        merchant_user_id: Option<uuid::Uuid>,
        merchant_name: Option<String>,
        merchant_created_at: Option<chrono::DateTime<chrono::Utc>>,
    }

    let row = sqlx::query_as::<_, Row>(
        "SELECT u.id, u.email, u.password_hash, u.name, u.is_admin,
                u.phone_number, u.phone_verified, u.created_at, u.updated_at,
                m.id AS merchant_id, m.user_id AS merchant_user_id,
                m.name AS merchant_name, m.created_at AS merchant_created_at
           FROM users u
           LEFT JOIN merchants m ON m.user_id = u.id
          WHERE u.id = $1
          LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;

    Ok(row.map(|r| {
        let user = User {
            id: r.id,
            email: r.email,
            password_hash: r.password_hash,
            name: r.name,
            is_admin: r.is_admin,
            phone_number: r.phone_number,
            phone_verified: r.phone_verified,
            created_at: r.created_at,
            updated_at: r.updated_at,
        };
        let merchant = match (
            r.merchant_id,
            r.merchant_user_id,
            r.merchant_name,
            r.merchant_created_at,
        ) {
            (Some(id), Some(user_id), Some(name), Some(created_at)) => Some(Merchant {
                id,
                user_id,
                name,
                created_at,
            }),
            _ => None,
        };
        (user, merchant)
    }))
}
```

## 2. Update `src/api/me.rs` handler

Replace the two awaits with:

```rust
let (user, merchant) = users::user_with_merchant(&state.db, auth.user_id)
    .await
    .map_err(internal)?
    .ok_or_else(|| not_found(ErrorCode::UserNotFound, "user not found"))?;
```

Response JSON is unchanged — `tests/me_join.rs` locks that contract.

## 3. Benchmark

```bash
DATABASE_URL=postgres://postgres:postgres@localhost:5432/aframp \
  ./scripts/bench_me_join.sh 500
```

On a sample of hundreds of users (each with one merchant) the JOIN typically
cuts wall-clock roughly in half versus two sequential queries over the same
pool, because it removes one network RTT per `/me` call.
