use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminUserRow {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub merchant_id: Option<Uuid>,
    pub merchant_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminMerchantRow {
    pub id: Uuid,
    pub name: String,
    pub owner_user_id: Uuid,
    pub owner_email: String,
    pub created_at: DateTime<Utc>,
    pub wallet_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminWalletRow {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub merchant_name: String,
    pub address: String,
    pub network: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminTransactionRow {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub merchant_name: String,
    pub wallet_address: String,
    pub tx_hash: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub network: String,
    pub status: String,
    pub confirmations: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminWithdrawalRow {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub merchant_name: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub status: String,
    pub provider: Option<String>,
    pub provider_reference: Option<String>,
    pub bank_code: Option<String>,
    pub account_number: Option<String>,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AdminPaymentRequestRow {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub merchant_name: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub memo: String,
    pub status: String,
    pub payment_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AssetTotal {
    pub asset: String,
    pub available: i64,
    pub pending: i64,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct StatusCount {
    pub status: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdminOverview {
    pub total_users: i64,
    pub total_merchants: i64,
    pub total_wallets: i64,
    pub balances_by_asset: Vec<AssetTotal>,
    pub payments_by_status: Vec<StatusCount>,
    pub withdrawals_by_status: Vec<StatusCount>,
    pub payment_requests_by_status: Vec<StatusCount>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for issue #1209: a merchant with multiple wallets must
    /// not be duplicated in the admin merchants listing. The query must return
    /// exactly one row per merchant, with a single deterministic wallet_address.
    #[sqlx::test]
    async fn admin_merchants_returns_one_row_per_merchant_with_multiple_wallets(
        pool: sqlx::PgPool,
    ) {
        let owner_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO users (id, email, name, is_admin, created_at)
            VALUES ($1, $2, $3, false, NOW())
            "#,
        )
        .bind(owner_id)
        .bind("owner@example.com")
        .bind("Owner")
        .execute(&pool)
        .await
        .expect("insert owner");

        let merchant_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO merchants (id, name, owner_user_id, created_at)
            VALUES ($1, $2, $3, NOW())
            "#,
        )
        .bind(merchant_id)
        .bind("Multi Wallet Merchant")
        .bind(owner_id)
        .execute(&pool)
        .await
        .expect("insert merchant");

        // Two wallets for the same merchant (the API does not prevent this).
        for address in ["GADDR_ONE", "GADDR_TWO"] {
            sqlx::query(
                r#"
                INSERT INTO wallets (id, merchant_id, address, network, created_at)
                VALUES ($1, $2, $3, $4, NOW())
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(merchant_id)
            .bind(address)
            .bind("testnet")
            .execute(&pool)
            .await
            .expect("insert wallet");
        }

        let rows: Vec<AdminMerchantRow> = crate::services::admin::merchants(&pool, None, None)
            .await
            .expect("admin merchants query");

        let matching: Vec<&AdminMerchantRow> =
            rows.iter().filter(|r| r.id == merchant_id).collect();

        assert_eq!(
            matching.len(),
            1,
            "merchant with multiple wallets must appear exactly once"
        );
        assert_eq!(
            matching[0].wallet_address.as_deref(),
            Some("GADDR_ONE"),
            "wallet_address must be deterministic (lowest address)"
        );
    }
}
