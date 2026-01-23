use crate::database::error::{DatabaseError, DatabaseErrorKind};
use crate::database::repository::{Repository, TransactionalRepository};
use async_trait::async_trait;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// Bill Payment entity extending a core transaction
#[derive(Debug, Clone, FromRow)]
pub struct BillPayment {
    pub id: Uuid,
    pub transaction_id: Uuid,
    pub provider_name: String,
    pub account_number: String,
    pub bill_type: String,
    pub due_date: Option<chrono::DateTime<chrono::Utc>>,
    pub paid_with_afri: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Repository for specifically managing bill payment details
pub struct BillPaymentRepository {
    pool: PgPool,
}

impl BillPaymentRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Find bill details by the core transaction ID
    pub async fn find_by_transaction_id(&self, transaction_id: Uuid) -> Result<Option<BillPayment>, DatabaseError> {
        sqlx::query_as::<_, BillPayment>(
            "SELECT id, transaction_id, provider_name, account_number, bill_type, due_date, paid_with_afri, created_at, updated_at 
             FROM bill_payments WHERE transaction_id = $1"
        )
        .bind(transaction_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    /// Create new bill payment details for a transaction
    pub async fn create_bill_payment(
        &self,
        transaction_id: Uuid,
        provider_name: &str,
        account_number: &str,
        bill_type: &str,
        due_date: Option<chrono::DateTime<chrono::Utc>>,
        paid_with_afri: bool,
    ) -> Result<BillPayment, DatabaseError> {
        sqlx::query_as::<_, BillPayment>(
            "INSERT INTO bill_payments (transaction_id, provider_name, account_number, bill_type, due_date, paid_with_afri) 
             VALUES ($1, $2, $3, $4, $5, $6) 
             RETURNING id, transaction_id, provider_name, account_number, bill_type, due_date, paid_with_afri, created_at, updated_at"
        )
        .bind(transaction_id)
        .bind(provider_name)
        .bind(account_number)
        .bind(bill_type)
        .bind(due_date)
        .bind(paid_with_afri)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }
}

#[async_trait]
impl Repository for BillPaymentRepository {
    type Entity = BillPayment;

    async fn find_by_id(&self, id: &str) -> Result<Option<Self::Entity>, DatabaseError> {
        let uuid = Uuid::parse_str(id).map_err(|e| DatabaseError::new(DatabaseErrorKind::Unknown { message: format!("Invalid UUID: {}", e) }))?;
        sqlx::query_as::<_, BillPayment>(
            "SELECT id, transaction_id, provider_name, account_number, bill_type, due_date, paid_with_afri, created_at, updated_at 
             FROM bill_payments WHERE id = $1"
        )
        .bind(uuid)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn find_all(&self) -> Result<Vec<Self::Entity>, DatabaseError> {
        sqlx::query_as::<_, BillPayment>(
            "SELECT id, transaction_id, provider_name, account_number, bill_type, due_date, paid_with_afri, created_at, updated_at 
             FROM bill_payments ORDER BY created_at DESC"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn insert(&self, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        sqlx::query_as::<_, BillPayment>(
            "INSERT INTO bill_payments (transaction_id, provider_name, account_number, bill_type, due_date, paid_with_afri) 
             VALUES ($1, $2, $3, $4, $5, $6) 
             RETURNING id, transaction_id, provider_name, account_number, bill_type, due_date, paid_with_afri, created_at, updated_at"
        )
        .bind(entity.transaction_id)
        .bind(&entity.provider_name)
        .bind(&entity.account_number)
        .bind(&entity.bill_type)
        .bind(entity.due_date)
        .bind(entity.paid_with_afri)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn update(&self, id: &str, entity: &Self::Entity) -> Result<Self::Entity, DatabaseError> {
        let uuid = Uuid::parse_str(id).map_err(|e| DatabaseError::new(DatabaseErrorKind::Unknown { message: format!("Invalid UUID: {}", e) }))?;
        sqlx::query_as::<_, BillPayment>(
            "UPDATE bill_payments 
             SET transaction_id = $1, provider_name = $2, account_number = $3, bill_type = $4, due_date = $5, paid_with_afri = $6 
             WHERE id = $7 
             RETURNING id, transaction_id, provider_name, account_number, bill_type, due_date, paid_with_afri, created_at, updated_at"
        )
        .bind(entity.transaction_id)
        .bind(&entity.provider_name)
        .bind(&entity.account_number)
        .bind(&entity.bill_type)
        .bind(entity.due_date)
        .bind(entity.paid_with_afri)
        .bind(uuid)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DatabaseError::from_sqlx(e))
    }

    async fn delete(&self, id: &str) -> Result<bool, DatabaseError> {
        let uuid = Uuid::parse_str(id).map_err(|e| DatabaseError::new(DatabaseErrorKind::Unknown { message: format!("Invalid UUID: {}", e) }))?;
        let result = sqlx::query("DELETE FROM bill_payments WHERE id = $1")
            .bind(uuid)
            .execute(&self.pool)
            .await
            .map_err(|e| DatabaseError::from_sqlx(e))?;
        Ok(result.rows_affected() > 0)
    }
}

impl TransactionalRepository for BillPaymentRepository {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
}
