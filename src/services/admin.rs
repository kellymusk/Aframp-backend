use sqlx::PgPool;

use crate::models::{
    AdminMerchantRow, AdminOverview, AdminPaymentRequestRow, AdminTransactionRow, AdminUserRow,
    AdminWalletRow, AdminWithdrawalRow, AssetTotal, StatusCount,
};

pub async fn overview(db: &PgPool) -> Result<AdminOverview, sqlx::Error> {
    let total_users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(db)
        .await?;
    let total_merchants: i64 = sqlx::query_scalar("SELECT count(*) FROM merchants")
        .fetch_one(db)
        .await?;
    let total_wallets: i64 = sqlx::query_scalar("SELECT count(*) FROM wallets")
        .fetch_one(db)
        .await?;

    let balances_by_asset = sqlx::query_as::<_, AssetTotal>(
        "SELECT asset, coalesce(sum(available), 0) AS available, coalesce(sum(pending), 0) AS pending
         FROM balances
         GROUP BY asset
         ORDER BY asset",
    )
    .fetch_all(db)
    .await?;

    let payments_by_status = sqlx::query_as::<_, StatusCount>(
        "SELECT status, count(*) FROM payments GROUP BY status ORDER BY status",
    )
    .fetch_all(db)
    .await?;

    let withdrawals_by_status = sqlx::query_as::<_, StatusCount>(
        "SELECT status, count(*) FROM withdrawals GROUP BY status ORDER BY status",
    )
    .fetch_all(db)
    .await?;

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

pub async fn users(db: &PgPool, limit: i64) -> Result<Vec<AdminUserRow>, sqlx::Error> {
    sqlx::query_as::<_, AdminUserRow>(
        "SELECT u.id, u.email, u.name, u.is_admin, u.created_at,
                m.id AS merchant_id, m.name AS merchant_name
         FROM users u
         LEFT JOIN merchants m ON m.user_id = u.id
         ORDER BY u.created_at DESC
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn merchants(db: &PgPool, limit: i64) -> Result<Vec<AdminMerchantRow>, sqlx::Error> {
    sqlx::query_as::<_, AdminMerchantRow>(
        "SELECT m.id, m.name, m.user_id AS owner_user_id, u.email AS owner_email, m.created_at,
                w.address AS wallet_address
         FROM merchants m
         JOIN users u ON u.id = m.user_id
         LEFT JOIN wallets w ON w.merchant_id = m.id
         ORDER BY m.created_at DESC
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn wallets(db: &PgPool, limit: i64) -> Result<Vec<AdminWalletRow>, sqlx::Error> {
    sqlx::query_as::<_, AdminWalletRow>(
        "SELECT w.id, w.merchant_id, m.name AS merchant_name, w.address, w.network, w.created_at
         FROM wallets w
         JOIN merchants m ON m.id = w.merchant_id
         ORDER BY w.created_at DESC
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn transactions(db: &PgPool, limit: i64) -> Result<Vec<AdminTransactionRow>, sqlx::Error> {
    sqlx::query_as::<_, AdminTransactionRow>(
        "SELECT p.id, p.merchant_id, m.name AS merchant_name, p.wallet_address, p.tx_hash,
                p.amount_stroops, p.asset, p.network, p.status, p.confirmations,
                p.created_at, p.updated_at
         FROM payments p
         JOIN merchants m ON m.id = p.merchant_id
         ORDER BY p.created_at DESC
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn withdrawals(db: &PgPool, limit: i64) -> Result<Vec<AdminWithdrawalRow>, sqlx::Error> {
    sqlx::query_as::<_, AdminWithdrawalRow>(
        "SELECT wd.id, wd.merchant_id, m.name AS merchant_name, wd.amount_stroops, wd.asset,
                wd.status, wd.provider, wd.provider_reference, wd.bank_code, wd.account_number,
                wd.failure_reason, wd.created_at, wd.updated_at
         FROM withdrawals wd
         JOIN merchants m ON m.id = wd.merchant_id
         ORDER BY wd.created_at DESC
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn payment_requests(db: &PgPool, limit: i64) -> Result<Vec<AdminPaymentRequestRow>, sqlx::Error> {
    sqlx::query_as::<_, AdminPaymentRequestRow>(
        "SELECT pr.id, pr.merchant_id, m.name AS merchant_name, pr.amount_stroops, pr.asset,
                pr.memo, pr.status, pr.payment_id, pr.expires_at, pr.created_at, pr.updated_at
         FROM payment_requests pr
         JOIN merchants m ON m.id = pr.merchant_id
         ORDER BY pr.created_at DESC
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}
