//! Database repository for the RTGS settlement rail

use sqlx::PgPool;
use uuid::Uuid;

use super::models::{
    ClearingHouseLedgerEntry, InterbankReconciliationLog, RtgsSettlementPool,
};

pub struct RtgsRepository {
    pool: PgPool,
}

impl RtgsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ── Settlement pools ──────────────────────────────────────────────────────

    pub async fn create_pool(
        &self,
        pool: &RtgsSettlementPool,
    ) -> Result<RtgsSettlementPool, anyhow::Error> {
        Ok(sqlx::query_as!(
            RtgsSettlementPool,
            r#"
            INSERT INTO rtgs_settlement_pools
                (id, bank_code, bank_name, currency, available_limit, net_debit_cap,
                 clearing_account_ref, is_active, created_at, updated_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,TRUE,NOW(),NOW())
            RETURNING *
            "#,
            pool.id,
            pool.bank_code,
            pool.bank_name,
            pool.currency,
            pool.available_limit,
            pool.net_debit_cap,
            pool.clearing_account_ref,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn get_pool_by_bank_code(
        &self,
        bank_code: &str,
    ) -> Result<Option<RtgsSettlementPool>, anyhow::Error> {
        Ok(sqlx::query_as!(
            RtgsSettlementPool,
            "SELECT * FROM rtgs_settlement_pools WHERE bank_code = $1 AND is_active = TRUE",
            bank_code
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_pools(&self) -> Result<Vec<RtgsSettlementPool>, anyhow::Error> {
        Ok(sqlx::query_as!(
            RtgsSettlementPool,
            "SELECT * FROM rtgs_settlement_pools WHERE is_active = TRUE ORDER BY bank_code"
        )
        .fetch_all(&self.pool)
        .await?)
    }

    // ── Ledger entries ────────────────────────────────────────────────────────

    pub async fn create_entry(
        &self,
        entry: &ClearingHouseLedgerEntry,
    ) -> Result<ClearingHouseLedgerEntry, anyhow::Error> {
        Ok(sqlx::query_as!(
            ClearingHouseLedgerEntry,
            r#"
            INSERT INTO clearing_house_ledger_entries
                (id, settlement_pool_id, bank_tracking_ref, amount, currency, direction,
                 status, two_pc_phase, aml_metadata, created_at, updated_at)
            VALUES ($1,$2,$3,$4,$5,$6,'PENDING','NONE',$7,NOW(),NOW())
            RETURNING *
            "#,
            entry.id,
            entry.settlement_pool_id,
            entry.bank_tracking_ref,
            entry.amount,
            entry.currency,
            entry.direction,
            entry.aml_metadata,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn get_entry(
        &self,
        id: Uuid,
    ) -> Result<Option<ClearingHouseLedgerEntry>, anyhow::Error> {
        Ok(sqlx::query_as!(
            ClearingHouseLedgerEntry,
            "SELECT * FROM clearing_house_ledger_entries WHERE id = $1",
            id
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn advance_two_pc(
        &self,
        id: Uuid,
        phase: &str,
    ) -> Result<ClearingHouseLedgerEntry, anyhow::Error> {
        Ok(sqlx::query_as!(
            ClearingHouseLedgerEntry,
            r#"
            UPDATE clearing_house_ledger_entries
            SET two_pc_phase = $2, updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
            id,
            phase,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn commit_entry(
        &self,
        id: Uuid,
        stellar_tx_hash: Option<&str>,
        stellar_ledger_sequence: Option<i64>,
    ) -> Result<ClearingHouseLedgerEntry, anyhow::Error> {
        Ok(sqlx::query_as!(
            ClearingHouseLedgerEntry,
            r#"
            UPDATE clearing_house_ledger_entries
            SET status = 'SETTLED',
                two_pc_phase = 'COMMIT',
                on_chain_tx_hash = $2,
                stellar_ledger_sequence = $3,
                settled_at = NOW(),
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
            id,
            stellar_tx_hash,
            stellar_ledger_sequence,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn reverse_entry(&self, id: Uuid) -> Result<ClearingHouseLedgerEntry, anyhow::Error> {
        Ok(sqlx::query_as!(
            ClearingHouseLedgerEntry,
            r#"
            UPDATE clearing_house_ledger_entries
            SET status = 'REVERSED',
                two_pc_phase = 'ABORT',
                reversed_at = NOW(),
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
            id,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn hold_for_reconciliation(
        &self,
        id: Uuid,
    ) -> Result<ClearingHouseLedgerEntry, anyhow::Error> {
        Ok(sqlx::query_as!(
            ClearingHouseLedgerEntry,
            r#"
            UPDATE clearing_house_ledger_entries
            SET status = 'HELD_FOR_RECONCILIATION', updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
            id,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    // ── Reconciliation logs ───────────────────────────────────────────────────

    pub async fn append_reconciliation_log(
        &self,
        log: &InterbankReconciliationLog,
    ) -> Result<(), anyhow::Error> {
        sqlx::query!(
            r#"
            INSERT INTO interbank_reconciliation_logs
                (id, ledger_entry_id, ack_code, nack_reason, message_type,
                 iso20022_payload, processing_node, duration_ms, occurred_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            "#,
            log.id,
            log.ledger_entry_id,
            log.ack_code,
            log.nack_reason,
            log.message_type,
            log.iso20022_payload,
            log.processing_node,
            log.duration_ms,
            log.occurred_at,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_reconciliation_logs(
        &self,
        entry_id: Uuid,
    ) -> Result<Vec<InterbankReconciliationLog>, anyhow::Error> {
        Ok(sqlx::query_as!(
            InterbankReconciliationLog,
            r#"
            SELECT id, ledger_entry_id, ack_code, nack_reason, message_type,
                   iso20022_payload, processing_node, duration_ms, occurred_at
            FROM interbank_reconciliation_logs
            WHERE ledger_entry_id = $1
            ORDER BY occurred_at ASC
            "#,
            entry_id,
        )
        .fetch_all(&self.pool)
        .await?)
    }
}
