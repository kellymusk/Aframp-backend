use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

/// A merchant-registered bank account.
///
/// Withdrawals must reference one of these rows (via `bank_account_id`) so a
/// merchant can only pay out to an account they have registered and verified.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct BankAccount {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub bank_code: String,
    pub account_number: String,
    pub account_name: String,
    pub is_verified: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl BankAccount {
    /// Fetch a single bank account scoped to its owning merchant.
    ///
    /// Returns `None` when the account does not exist or belongs to a
    /// different merchant, preventing cross-merchant withdrawals.
    pub async fn find_for_merchant(
        pool: &sqlx::PgPool,
        merchant_id: Uuid,
        bank_account_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT id, merchant_id, bank_code, account_number, account_name,
                   is_verified, created_at, updated_at
            FROM bank_accounts
            WHERE id = $1 AND merchant_id = $2
            "#,
        )
        .bind(bank_account_id)
        .bind(merchant_id)
        .fetch_optional(pool)
        .await
    }

    /// List all bank accounts registered by a merchant.
    pub async fn list_for_merchant(
        pool: &sqlx::PgPool,
        merchant_id: Uuid,
    ) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT id, merchant_id, bank_code, account_number, account_name,
                   is_verified, created_at, updated_at
            FROM bank_accounts
            WHERE merchant_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(merchant_id)
        .fetch_all(pool)
        .await
    }

    /// Insert a newly registered (and Paystack-resolved) bank account.
    pub async fn create(
        pool: &sqlx::PgPool,
        merchant_id: Uuid,
        bank_code: &str,
        account_number: &str,
        account_name: &str,
        is_verified: bool,
    ) -> Result<Self, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            INSERT INTO bank_accounts
                (merchant_id, bank_code, account_number, account_name, is_verified)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, merchant_id, bank_code, account_number, account_name,
                      is_verified, created_at, updated_at
            "#,
        )
        .bind(merchant_id)
        .bind(bank_code)
        .bind(account_number)
        .bind(account_name)
        .bind(is_verified)
        .fetch_one(pool)
        .await
    }
}
