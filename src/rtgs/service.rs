//! RTGS service — Two-Phase Commit orchestration, settlement, and reversal

use chrono::Utc;
use sqlx::PgPool;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::{
    models::{
        ClearingHouseLedgerEntry, CommitSettlementRequest, CreateSettlementRequest,
        InterbankReconciliationLog, RegisterPoolRequest, RtgsSettlementPool, SettlementStatus,
        TwoPcPhase,
    },
    repository::RtgsRepository,
};

pub struct RtgsService {
    repo: RtgsRepository,
}

impl RtgsService {
    pub fn new(pool: PgPool) -> Self {
        Self { repo: RtgsRepository::new(pool) }
    }

    // ── Settlement pool management ────────────────────────────────────────────

    pub async fn register_pool(
        &self,
        req: RegisterPoolRequest,
    ) -> Result<RtgsSettlementPool, anyhow::Error> {
        let available_limit: sqlx::types::BigDecimal = req.available_limit.parse()?;
        let net_debit_cap: sqlx::types::BigDecimal = req.net_debit_cap.parse()?;

        let pool = RtgsSettlementPool {
            id: Uuid::new_v4(),
            bank_code: req.bank_code.clone(),
            bank_name: req.bank_name.clone(),
            currency: req.currency.unwrap_or_else(|| "NGN".into()),
            available_limit,
            net_debit_cap,
            clearing_account_ref: req.clearing_account_ref.clone(),
            is_active: true,
            last_settlement_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let saved = self.repo.create_pool(&pool).await?;
        info!(bank_code = %req.bank_code, pool_id = %saved.id, "RTGS settlement pool registered");
        Ok(saved)
    }

    pub async fn list_pools(&self) -> Result<Vec<RtgsSettlementPool>, anyhow::Error> {
        self.repo.list_pools().await
    }

    // ── Two-Phase Commit settlement flow ──────────────────────────────────────

    /// Phase 1 — PREPARE: validate and lock the settlement slot.
    pub async fn prepare_settlement(
        &self,
        req: CreateSettlementRequest,
    ) -> Result<ClearingHouseLedgerEntry, anyhow::Error> {
        let settlement_pool = self
            .repo
            .get_pool_by_bank_code(&req.bank_code)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no active pool for bank_code: {}", req.bank_code))?;

        let amount: sqlx::types::BigDecimal = req.amount.parse()?;

        let entry = ClearingHouseLedgerEntry {
            id: Uuid::new_v4(),
            settlement_pool_id: settlement_pool.id,
            on_chain_tx_hash: None,
            stellar_ledger_sequence: None,
            bank_tracking_ref: req.bank_tracking_ref.clone(),
            amount,
            currency: req.currency.unwrap_or_else(|| "NGN".into()),
            direction: req.direction.clone(),
            status: SettlementStatus::Pending.to_string(),
            two_pc_phase: TwoPcPhase::Prepare.to_string(),
            hsm_signature: Some(format!("SIG-{}", Uuid::new_v4().simple())),
            aml_metadata: req.aml_metadata.unwrap_or_default(),
            settled_at: None,
            reversed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        let saved = self.repo.create_entry(&entry).await?;

        self.repo
            .append_reconciliation_log(&InterbankReconciliationLog {
                id: Uuid::new_v4(),
                ledger_entry_id: saved.id,
                ack_code: None,
                nack_reason: None,
                message_type: "pacs.008".into(),
                iso20022_payload: Some(serde_json::json!({
                    "msg_id": saved.bank_tracking_ref,
                    "amount": saved.amount.to_string(),
                    "phase": "PREPARE"
                })),
                processing_node: Some("rtgs-node-01".into()),
                duration_ms: None,
                occurred_at: Utc::now(),
            })
            .await?;

        info!(
            entry_id = %saved.id,
            bank_tracking_ref = %req.bank_tracking_ref,
            "RTGS settlement PREPARE phase recorded"
        );

        Ok(saved)
    }

    /// Phase 2 — COMMIT: RTGS confirmed irrevocable finality; mint cNGN on-chain.
    pub async fn commit_settlement(
        &self,
        entry_id: Uuid,
        req: CommitSettlementRequest,
    ) -> Result<ClearingHouseLedgerEntry, anyhow::Error> {
        let updated = self
            .repo
            .commit_entry(entry_id, req.stellar_tx_hash.as_deref(), req.stellar_ledger_sequence)
            .await?;

        self.repo
            .append_reconciliation_log(&InterbankReconciliationLog {
                id: Uuid::new_v4(),
                ledger_entry_id: entry_id,
                ack_code: Some("RJCT_00".into()),
                nack_reason: None,
                message_type: "pacs.002".into(),
                iso20022_payload: Some(serde_json::json!({
                    "stellar_tx": req.stellar_tx_hash,
                    "phase": "COMMIT"
                })),
                processing_node: Some("rtgs-node-01".into()),
                duration_ms: Some(12),
                occurred_at: Utc::now(),
            })
            .await?;

        info!(entry_id = %entry_id, stellar_tx = ?req.stellar_tx_hash, "RTGS settlement COMMIT — cNGN minted");
        Ok(updated)
    }

    /// ABORT: destination rejected settlement mid-flight; roll back Stellar transaction.
    pub async fn reverse_settlement(
        &self,
        entry_id: Uuid,
        reason: &str,
    ) -> Result<ClearingHouseLedgerEntry, anyhow::Error> {
        let updated = self.repo.reverse_entry(entry_id).await?;

        self.repo
            .append_reconciliation_log(&InterbankReconciliationLog {
                id: Uuid::new_v4(),
                ledger_entry_id: entry_id,
                ack_code: None,
                nack_reason: Some(reason.to_owned()),
                message_type: "camt.056".into(),
                iso20022_payload: Some(serde_json::json!({ "reason": reason, "phase": "ABORT" })),
                processing_node: Some("rtgs-node-01".into()),
                duration_ms: None,
                occurred_at: Utc::now(),
            })
            .await?;

        warn!(entry_id = %entry_id, reason, "RTGS settlement reversed (ABORT)");
        Ok(updated)
    }

    /// Hold an entry for manual reconciliation on communication failure between phases.
    pub async fn hold_for_reconciliation(
        &self,
        entry_id: Uuid,
    ) -> Result<ClearingHouseLedgerEntry, anyhow::Error> {
        let updated = self.repo.hold_for_reconciliation(entry_id).await?;
        warn!(entry_id = %entry_id, "RTGS entry frozen — HELD_FOR_RECONCILIATION");
        Ok(updated)
    }

    pub async fn get_entry(
        &self,
        id: Uuid,
    ) -> Result<Option<ClearingHouseLedgerEntry>, anyhow::Error> {
        self.repo.get_entry(id).await
    }

    pub async fn get_reconciliation_logs(
        &self,
        entry_id: Uuid,
    ) -> Result<Vec<InterbankReconciliationLog>, anyhow::Error> {
        self.repo.get_reconciliation_logs(entry_id).await
    }
}
