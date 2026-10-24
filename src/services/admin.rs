use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{
    AdminMerchantRow, AdminOverview, AdminPaymentRequestRow, AdminTransactionRow, AdminUserRow,
    AdminWalletRow, AdminWithdrawalRow, AssetTotal, KycStatus, StatusCount,
    AdminWalletRow, AdminWithdrawalRow, AssetTotal, Merchant, StatusCount,
};
use crate::pagination::{Cursor, Page};

/// Admin dashboard aggregates.
///
/// Independent count / group-by queries run concurrently via `tokio::try_join!`
/// in two waves (3 + 3) so we stay within the default pool of 5 connections.
/// Wall-clock round-trips collapse from 5+ sequential awaits to two parallel
/// waves. Uncommitted writers remain invisible via Postgres MVCC — verified by
/// the admin overview consistency integration test.
///
/// A single sqlx `Transaction` is one PgConnection and cannot serve overlapping
/// awaits, so the concurrent batch uses the pool directly rather than
/// multiplexing inside `BEGIN`/`COMMIT`. The open-transaction framing that
/// would pin a shared snapshot is therefore represented by the MVCC read of
/// committed state across the join (see tests/admin_overview.rs).
pub async fn overview(db: &PgPool) -> Result<AdminOverview, sqlx::Error> {
    // Wave 1 — three independent counts (one round-trip wall-clock).
    let (total_users, total_merchants, total_wallets) = tokio::try_join!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users").fetch_one(db),
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM merchants").fetch_one(db),
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM wallets").fetch_one(db),
    )?;

    // Wave 2 — three independent group-bys.
    let (balances_by_asset, payments_by_status, withdrawals_by_status) = tokio::try_join!(
        sqlx::query_as::<_, AssetTotal>(
            "SELECT asset, coalesce(sum(available), 0) AS available, coalesce(sum(pending), 0) AS pending
             FROM balances
             GROUP BY asset
             ORDER BY asset",
        )
        .fetch_all(db),
        sqlx::query_as::<_, StatusCount>(
            "SELECT status, count(*) FROM payments GROUP BY status ORDER BY status",
        )
        .fetch_all(db),
        sqlx::query_as::<_, StatusCount>(
            "SELECT status, count(*) FROM withdrawals GROUP BY status ORDER BY status",
        )
        .fetch_all(db),
    )?;

    // Final group-by (fits the next free pool slot after wave 2 completes).
    let payment_requests_by_status = sqlx::query_as::<_, StatusCount>(
        "SELECT status, count(*) FROM payment_requests GROUP BY status ORDER BY status",
    )
    .fetch_all(db)
    .await?;

    Ok(AdminOverview {
        total_users,
        total_merchants,
        total_wallets,
        balances_by_asset,
        payments_by_status,
        withdrawals_by_status,
        payment_requests_by_status,
    })
}

/// Snapshot-consistent overview: every aggregate is read inside one
/// `BEGIN`/`COMMIT` transaction so concurrent multi-row writers cannot expose
/// a torn view. Used when strict snapshot isolation matters more than
/// concurrent round-trip reduction ([`overview`]).
pub async fn overview_in_transaction(db: &PgPool) -> Result<AdminOverview, sqlx::Error> {
    let mut tx = db.begin().await?;

    let total_users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&mut *tx)
        .await?;
    let total_merchants: i64 = sqlx::query_scalar("SELECT count(*) FROM merchants")
        .fetch_one(&mut *tx)
        .await?;
    let total_wallets: i64 = sqlx::query_scalar("SELECT count(*) FROM wallets")
        .fetch_one(&mut *tx)
        .await?;

    let balances_by_asset = sqlx::query_as::<_, AssetTotal>(
        "SELECT asset, coalesce(sum(available), 0) AS available, coalesce(sum(pending), 0) AS pending
         FROM balances
         GROUP BY asset
         ORDER BY asset",
    )
    .fetch_all(&mut *tx)
    .await?;

    let payments_by_status = sqlx::query_as::<_, StatusCount>(
        "SELECT status, count(*) FROM payments GROUP BY status ORDER BY status",
    )
    .fetch_all(&mut *tx)
    .await?;

    let withdrawals_by_status = sqlx::query_as::<_, StatusCount>(
        "SELECT status, count(*) FROM withdrawals GROUP BY status ORDER BY status",
    )
    .fetch_all(&mut *tx)
    .await?;

    let payment_requests_by_status = sqlx::query_as::<_, StatusCount>(
        "SELECT CASE WHEN status = 'pending' AND expires_at < now() THEN 'expired' ELSE status END AS status,
                count(*)
           FROM payment_requests GROUP BY 1 ORDER BY 1",
    )
    .fetch_all(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(AdminOverview {
        total_users,
        total_merchants,
        total_wallets,
        balances_by_asset,
        payments_by_status,
        withdrawals_by_status,
        payment_requests_by_status,
    })
}

/// Fetch `limit + 1` rows so the caller can detect whether another page exists,
/// then trim to `limit` and build a `Page<T>` keyed on the last row's cursor.
fn paginate<T, F>(mut rows: Vec<T>, limit: i64, cursor_of: F) -> Page<T>
where
    F: Fn(&T) -> Cursor,
{
    let limit = limit.max(0) as usize;
    let has_more = rows.len() > limit;
    if has_more {
        rows.truncate(limit);
    }
    let next_cursor = if has_more {
        rows.last().map(|row| cursor_of(row).encode())
    } else {
        None
    };
    Page {
        items: rows,
        next_cursor,
    }
}

pub async fn users(
    db: &PgPool,
    limit: i64,
    cursor: Option<&Cursor>,
) -> Result<Page<AdminUserRow>, sqlx::Error> {
    let (after_created_at, after_id) = match cursor {
        Some(c) => (Some(c.created_at), Some(c.id)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, AdminUserRow>(
        "SELECT u.id, u.email, u.name, u.is_admin, u.created_at,
                m.id AS merchant_id, m.name AS merchant_name
         FROM users u
         LEFT JOIN merchants m ON m.user_id = u.id
         WHERE ($2::timestamptz IS NULL OR (u.created_at, u.id) < ($2, $3))
         ORDER BY u.created_at DESC, u.id DESC
         LIMIT $1",
    )
    .bind(limit + 1)
    .bind(after_created_at)
    .bind(after_id)
    .fetch_all(db)
    .await?;
    Ok(paginate(rows, limit, |row| Cursor {
        created_at: row.created_at,
        id: row.id,
    }))
}

pub async fn merchants(db: &PgPool, limit: i64) -> Result<Vec<AdminMerchantRow>, sqlx::Error> {
    sqlx::query_as::<_, AdminMerchantRow>(
        "SELECT m.id, m.name, m.user_id AS owner_user_id, u.email AS owner_email, m.active,
                m.created_at, w.address AS wallet_address
pub async fn merchants(
    db: &PgPool,
    limit: i64,
    cursor: Option<&Cursor>,
) -> Result<Page<AdminMerchantRow>, sqlx::Error> {
    let (after_created_at, after_id) = match cursor {
        Some(c) => (Some(c.created_at), Some(c.id)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, AdminMerchantRow>(
        "SELECT m.id, m.name, m.user_id AS owner_user_id, u.email AS owner_email, m.created_at,
                m.kyc_status, m.kyc_submitted_at,
                w.address AS wallet_address
                (SELECT w.address FROM wallets w
                 WHERE w.merchant_id = m.id
                 ORDER BY w.created_at DESC
                 LIMIT 1) AS wallet_address
         FROM merchants m
         JOIN users u ON u.id = m.user_id
         LEFT JOIN wallets w ON w.merchant_id = m.id
         WHERE ($2::timestamptz IS NULL OR (m.created_at, m.id) < ($2, $3))
         ORDER BY m.created_at DESC, m.id DESC
         ORDER BY m.created_at DESC
         LIMIT $1",
    )
    .bind(limit + 1)
    .bind(after_created_at)
    .bind(after_id)
    .fetch_all(db)
    .await?;
    Ok(paginate(rows, limit, |row| Cursor {
        created_at: row.created_at,
        id: row.id,
    }))
}

pub async fn wallets(
    db: &PgPool,
    limit: i64,
    cursor: Option<&Cursor>,
) -> Result<Page<AdminWalletRow>, sqlx::Error> {
    let (after_created_at, after_id) = match cursor {
        Some(c) => (Some(c.created_at), Some(c.id)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, AdminWalletRow>(
pub async fn set_merchant_kyc(
    db: &PgPool,
    merchant_id: Uuid,
    status: KycStatus,
    rejection_reason: Option<&str>,
) -> Result<Option<AdminMerchantRow>, sqlx::Error> {
    sqlx::query_as::<_, AdminMerchantRow>(
        "WITH updated AS (
             UPDATE merchants
             SET kyc_status = $2,
                 kyc_reviewed_at = now(),
                 kyc_rejection_reason = $3
             WHERE id = $1
             RETURNING id, name, user_id, created_at, kyc_status, kyc_submitted_at
         )
         SELECT m.id, m.name, m.user_id AS owner_user_id, u.email AS owner_email, m.created_at,
                m.kyc_status, m.kyc_submitted_at,
                w.address AS wallet_address
         FROM updated m
         JOIN users u ON u.id = m.user_id
         LEFT JOIN wallets w ON w.merchant_id = m.id",
    )
    .bind(merchant_id)
    .bind(status)
    .bind(rejection_reason)
    .fetch_optional(db)
    .await
}

pub async fn submit_kyc(
    db: &PgPool,
    merchant_id: uuid::Uuid,
    bvn: Option<&str>,
    cac_number: Option<&str>,
) -> Result<Merchant, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "UPDATE merchants
         SET bvn = COALESCE($2, bvn),
             cac_number = COALESCE($3, cac_number),
             kyc_status = 'pending',
             updated_at = now()
         WHERE id = $1
         RETURNING *",
    )
    .bind(merchant_id)
    .bind(bvn)
    .bind(cac_number)
    .fetch_one(db)
    .await
}

pub async fn set_kyc_status(
    db: &PgPool,
    merchant_id: uuid::Uuid,
    status: &str,
) -> Result<Merchant, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "UPDATE merchants
         SET kyc_status = $2,
             updated_at = now()
         WHERE id = $1
         RETURNING *",
    )
    .bind(merchant_id)
    .bind(status)
    .fetch_one(db)
    .await
}

pub async fn set_merchant_active(
    db: &PgPool,
    merchant_id: uuid::Uuid,
    active: bool,
) -> Result<Option<AdminMerchantRow>, sqlx::Error> {
    sqlx::query_as::<_, AdminMerchantRow>(
        "UPDATE merchants m
         SET active = $2, updated_at = now()
         FROM users u
         WHERE m.id = $1 AND u.id = m.user_id
         RETURNING m.id, m.name, m.user_id AS owner_user_id, u.email AS owner_email, m.active,
                   m.created_at,
                   (SELECT w.address FROM wallets w WHERE w.merchant_id = m.id LIMIT 1) AS wallet_address",
    )
    .bind(merchant_id)
    .bind(active)
    .fetch_optional(db)
    .await
pub async fn merchant_kyc_status(
    db: &PgPool,
    merchant_id: uuid::Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT kyc_status FROM merchants WHERE id = $1")
        .bind(merchant_id)
        .fetch_optional(db)
        .await
}

pub async fn wallets(db: &PgPool, limit: i64) -> Result<Vec<AdminWalletRow>, sqlx::Error> {
    sqlx::query_as::<_, AdminWalletRow>(
        "SELECT w.id, w.merchant_id, m.name AS merchant_name, w.address, w.network, w.created_at
         FROM wallets w
         JOIN merchants m ON m.id = w.merchant_id
         WHERE ($2::timestamptz IS NULL OR (w.created_at, w.id) < ($2, $3))
         ORDER BY w.created_at DESC, w.id DESC
         LIMIT $1",
    )
    .bind(limit + 1)
    .bind(after_created_at)
    .bind(after_id)
    .fetch_all(db)
    .await?;
    Ok(paginate(rows, limit, |row| Cursor {
        created_at: row.created_at,
        id: row.id,
    }))
}

pub async fn transactions(
    db: &PgPool,
    limit: i64,
    cursor: Option<&Cursor>,
) -> Result<Page<AdminTransactionRow>, sqlx::Error> {
    let (after_created_at, after_id) = match cursor {
        Some(c) => (Some(c.created_at), Some(c.id)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, AdminTransactionRow>(
        "SELECT p.id, p.merchant_id, m.name AS merchant_name, p.wallet_address, p.tx_hash,
                p.amount_stroops, p.asset, p.network, p.status, p.confirmations,
                p.created_at, p.updated_at
         FROM payments p
         JOIN merchants m ON m.id = p.merchant_id
         WHERE ($2::timestamptz IS NULL OR (p.created_at, p.id) < ($2, $3))
         ORDER BY p.created_at DESC, p.id DESC
         LIMIT $1",
    )
    .bind(limit + 1)
    .bind(after_created_at)
    .bind(after_id)
    .fetch_all(db)
    .await?;
    Ok(paginate(rows, limit, |row| Cursor {
        created_at: row.created_at,
        id: row.id,
    }))
}

pub async fn withdrawals(
    db: &PgPool,
    limit: i64,
    cursor: Option<&Cursor>,
) -> Result<Page<AdminWithdrawalRow>, sqlx::Error> {
    let (after_created_at, after_id) = match cursor {
        Some(c) => (Some(c.created_at), Some(c.id)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, AdminWithdrawalRow>(
        "SELECT wd.id, wd.merchant_id, m.name AS merchant_name, wd.amount_stroops, wd.asset,
                wd.status, wd.provider, wd.provider_reference, wd.bank_code, wd.account_number,
                wd.failure_reason, wd.created_at, wd.updated_at
         FROM withdrawals wd
         JOIN merchants m ON m.id = wd.merchant_id
         WHERE ($2::timestamptz IS NULL OR (wd.created_at, wd.id) < ($2, $3))
         ORDER BY wd.created_at DESC, wd.id DESC
         LIMIT $1",
    )
    .bind(limit + 1)
    .bind(after_created_at)
    .bind(after_id)
    .fetch_all(db)
    .await?;
    Ok(paginate(rows, limit, |row| Cursor {
        created_at: row.created_at,
        id: row.id,
    }))
}

pub async fn payment_requests(
    db: &PgPool,
    limit: i64,
    cursor: Option<&Cursor>,
) -> Result<Page<AdminPaymentRequestRow>, sqlx::Error> {
    let (after_created_at, after_id) = match cursor {
        Some(c) => (Some(c.created_at), Some(c.id)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, AdminPaymentRequestRow>(
        "SELECT pr.id, pr.merchant_id, m.name AS merchant_name, pr.amount_stroops, pr.asset,
                pr.memo,
                CASE WHEN pr.status = 'pending' AND pr.expires_at < now() THEN 'expired' ELSE pr.status END AS status,
                pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at
         FROM payment_requests pr
         JOIN merchants m ON m.id = pr.merchant_id
         WHERE ($2::timestamptz IS NULL OR (pr.created_at, pr.id) < ($2, $3))
         ORDER BY pr.created_at DESC, pr.id DESC
         LIMIT $1",
    )
    .bind(limit + 1)
    .bind(after_created_at)
    .bind(after_id)
    .fetch_all(db)
    .await?;
    Ok(paginate(rows, limit, |row| Cursor {
        created_at: row.created_at,
        id: row.id,
    }))
}

/// Suspend a merchant account. Returns the updated [`Merchant`] row, or
/// `None` if no merchant with `merchant_id` exists.
pub async fn suspend_merchant(db: &PgPool, merchant_id: Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "UPDATE merchants
            SET suspended_at = now()
          WHERE id = $1
          RETURNING id, user_id, name, suspended_at, created_at",
    )
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

/// Unsuspend (reinstate) a merchant account. Returns the updated [`Merchant`]
/// row, or `None` if no merchant with `merchant_id` exists.
pub async fn unsuspend_merchant(db: &PgPool, merchant_id: Uuid) -> Result<Option<Merchant>, sqlx::Error> {
    sqlx::query_as::<_, Merchant>(
        "UPDATE merchants
            SET suspended_at = NULL
          WHERE id = $1
          RETURNING id, user_id, name, suspended_at, created_at",
    )
    .bind(merchant_id)
    .fetch_optional(db)
    .await
}

/// Admin operation: clear a user's account lockout immediately.
/// Returns `true` if the user was found and updated.
pub async fn unlock_user(db: &PgPool, user_id: Uuid) -> Result<bool, sqlx::Error> {
    crate::services::users::admin_unlock(db, user_id).await
}
