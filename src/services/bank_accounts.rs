use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// A merchant-registered bank account. Ownership is enforced by scoping every
/// query to the owning merchant, and verification is performed by resolving
/// the account through Paystack before it can be used for withdrawals.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct BankAccount {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub bank_code: String,
    pub account_number: String,
    pub account_name: String,
    pub is_verified: bool,
}

#[derive(Debug, Deserialize)]
pub struct RegisterBankAccountRequest {
    pub bank_code: String,
    pub account_number: String,
}

#[derive(Debug)]
pub enum BankAccountError {
    InvalidInput(String),
    ResolutionFailed(String),
    Database(sqlx::Error),
}

impl From<sqlx::Error> for BankAccountError {
    fn from(err: sqlx::Error) -> Self {
        BankAccountError::Database(err)
    }
}

/// Resolved account details returned by Paystack account resolution.
#[derive(Debug, Clone)]
pub struct ResolvedAccount {
    pub account_name: String,
}

/// Resolve a bank account via Paystack to verify it exists and belongs to the
/// merchant. Implementations should call Paystack's `/bank/resolve` endpoint.
#[async_trait::async_trait]
pub trait AccountResolver: Send + Sync {
    async fn resolve(
        &self,
        bank_code: &str,
        account_number: &str,
    ) -> Result<ResolvedAccount, String>;
}

pub struct BankAccountService;

impl BankAccountService {
    /// Register a bank account for a merchant, verifying it via Paystack
    /// account resolution before persisting it.
    pub async fn register(
        pool: &PgPool,
        resolver: &dyn AccountResolver,
        merchant_id: Uuid,
        req: RegisterBankAccountRequest,
    ) -> Result<BankAccount, BankAccountError> {
        let bank_code = req.bank_code.trim();
        let account_number = req.account_number.trim();

        if bank_code.is_empty() {
            return Err(BankAccountError::InvalidInput(
                "bank_code is required".to_string(),
            ));
        }
        if account_number.len() != 10 || !account_number.chars().all(|c| c.is_ascii_digit()) {
            return Err(BankAccountError::InvalidInput(
                "account_number must be 10 digits".to_string(),
            ));
        }

        let resolved = resolver
            .resolve(bank_code, account_number)
            .await
            .map_err(BankAccountError::ResolutionFailed)?;

        let account = sqlx::query_as::<_, BankAccount>(
            r#"
            INSERT INTO bank_accounts
                (merchant_id, bank_code, account_number, account_name, is_verified)
            VALUES ($1, $2, $3, $4, TRUE)
            ON CONFLICT (merchant_id, bank_code, account_number)
            DO UPDATE SET account_name = EXCLUDED.account_name, is_verified = TRUE
            RETURNING id, merchant_id, bank_code, account_number, account_name, is_verified
            "#,
        )
        .bind(merchant_id)
        .bind(bank_code)
        .bind(account_number)
        .bind(&resolved.account_name)
        .fetch_one(pool)
        .await?;

        Ok(account)
    }

    /// List all bank accounts owned by a merchant.
    pub async fn list_for_merchant(
        pool: &PgPool,
        merchant_id: Uuid,
    ) -> Result<Vec<BankAccount>, BankAccountError> {
        let accounts = sqlx::query_as::<_, BankAccount>(
            r#"
            SELECT id, merchant_id, bank_code, account_number, account_name, is_verified
            FROM bank_accounts
            WHERE merchant_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(merchant_id)
        .fetch_all(pool)
        .await?;

        Ok(accounts)
    }

    /// Fetch a bank account by id, scoped to the owning merchant. Returns
    /// `None` when the account does not exist or belongs to another merchant,
    /// preventing cross-merchant withdrawals.
    pub async fn get_for_merchant(
        pool: &PgPool,
        merchant_id: Uuid,
        bank_account_id: Uuid,
    ) -> Result<Option<BankAccount>, BankAccountError> {
        let account = sqlx::query_as::<_, BankAccount>(
            r#"
            SELECT id, merchant_id, bank_code, account_number, account_name, is_verified
            FROM bank_accounts
            WHERE id = $1 AND merchant_id = $2 AND is_verified = TRUE
            "#,
        )
        .bind(bank_account_id)
        .bind(merchant_id)
        .fetch_optional(pool)
        .await?;

        Ok(account)
    }
}
