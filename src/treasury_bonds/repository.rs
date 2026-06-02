//! Database repository for tokenized treasury bond rails

use sqlx::PgPool;
use uuid::Uuid;

use super::models::{AutomatedSweepPolicy, BondLedgerAllocation, TokenizedBondInstrument};

pub struct TreasuryBondsRepository {
    pool: PgPool,
}

impl TreasuryBondsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ── Bond instruments ──────────────────────────────────────────────────────

    pub async fn create_instrument(
        &self,
        inst: &TokenizedBondInstrument,
    ) -> Result<TokenizedBondInstrument, anyhow::Error> {
        Ok(sqlx::query_as!(
            TokenizedBondInstrument,
            r#"
            INSERT INTO tokenized_bond_instruments
                (id, isin, issuer_authority, instrument_name, currency, face_value,
                 coupon_rate_bps, maturity_at, auction_date, status, metadata, created_at, updated_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'ACTIVE','{}',NOW(),NOW())
            RETURNING *
            "#,
            inst.id,
            inst.isin,
            inst.issuer_authority,
            inst.instrument_name,
            inst.currency,
            inst.face_value,
            inst.coupon_rate_bps,
            inst.maturity_at,
            inst.auction_date,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn get_instrument(
        &self,
        id: Uuid,
    ) -> Result<Option<TokenizedBondInstrument>, anyhow::Error> {
        Ok(sqlx::query_as!(
            TokenizedBondInstrument,
            "SELECT * FROM tokenized_bond_instruments WHERE id = $1",
            id
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_active_instruments(
        &self,
    ) -> Result<Vec<TokenizedBondInstrument>, anyhow::Error> {
        Ok(sqlx::query_as!(
            TokenizedBondInstrument,
            "SELECT * FROM tokenized_bond_instruments WHERE status = 'ACTIVE' ORDER BY maturity_at"
        )
        .fetch_all(&self.pool)
        .await?)
    }

    // ── Allocations ───────────────────────────────────────────────────────────

    pub async fn create_allocation(
        &self,
        alloc: &BondLedgerAllocation,
    ) -> Result<BondLedgerAllocation, anyhow::Error> {
        Ok(sqlx::query_as!(
            BondLedgerAllocation,
            r#"
            INSERT INTO bond_ledger_allocations
                (id, tenant_id, bond_instrument_id, fractional_units, purchase_price,
                 accrued_yield, status, acquired_at, created_at, updated_at)
            VALUES ($1,$2,$3,$4,$5,0,'ACTIVE',$6,NOW(),NOW())
            RETURNING *
            "#,
            alloc.id,
            alloc.tenant_id,
            alloc.bond_instrument_id,
            alloc.fractional_units,
            alloc.purchase_price,
            alloc.acquired_at,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn get_allocation(
        &self,
        id: Uuid,
    ) -> Result<Option<BondLedgerAllocation>, anyhow::Error> {
        Ok(sqlx::query_as!(
            BondLedgerAllocation,
            "SELECT * FROM bond_ledger_allocations WHERE id = $1",
            id
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_tenant_allocations(
        &self,
        tenant_id: Uuid,
    ) -> Result<Vec<BondLedgerAllocation>, anyhow::Error> {
        Ok(sqlx::query_as!(
            BondLedgerAllocation,
            "SELECT * FROM bond_ledger_allocations WHERE tenant_id = $1 ORDER BY acquired_at DESC",
            tenant_id
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn liquidate_allocation(&self, id: Uuid) -> Result<BondLedgerAllocation, anyhow::Error> {
        Ok(sqlx::query_as!(
            BondLedgerAllocation,
            r#"
            UPDATE bond_ledger_allocations
            SET status = 'LIQUIDATED', redeemed_at = NOW(), updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
            id
        )
        .fetch_one(&self.pool)
        .await?)
    }

    // ── Sweep policies ────────────────────────────────────────────────────────

    pub async fn upsert_sweep_policy(
        &self,
        policy: &AutomatedSweepPolicy,
    ) -> Result<AutomatedSweepPolicy, anyhow::Error> {
        Ok(sqlx::query_as!(
            AutomatedSweepPolicy,
            r#"
            INSERT INTO automated_sweep_policies
                (id, tenant_id, enabled, min_sweep_threshold_ngn, max_portfolio_duration_days,
                 preferred_instrument_id, sweep_interval_minutes, created_at, updated_at)
            VALUES ($1,$2,$3,$4,$5,$6,$7,NOW(),NOW())
            ON CONFLICT (tenant_id) DO UPDATE
            SET enabled = EXCLUDED.enabled,
                min_sweep_threshold_ngn = EXCLUDED.min_sweep_threshold_ngn,
                max_portfolio_duration_days = EXCLUDED.max_portfolio_duration_days,
                preferred_instrument_id = EXCLUDED.preferred_instrument_id,
                sweep_interval_minutes = EXCLUDED.sweep_interval_minutes,
                updated_at = NOW()
            RETURNING *
            "#,
            policy.id,
            policy.tenant_id,
            policy.enabled,
            policy.min_sweep_threshold_ngn,
            policy.max_portfolio_duration_days,
            policy.preferred_instrument_id,
            policy.sweep_interval_minutes,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn get_sweep_policy(
        &self,
        tenant_id: Uuid,
    ) -> Result<Option<AutomatedSweepPolicy>, anyhow::Error> {
        Ok(sqlx::query_as!(
            AutomatedSweepPolicy,
            "SELECT * FROM automated_sweep_policies WHERE tenant_id = $1",
            tenant_id
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list_due_sweeps(&self) -> Result<Vec<AutomatedSweepPolicy>, anyhow::Error> {
        Ok(sqlx::query_as!(
            AutomatedSweepPolicy,
            r#"
            SELECT * FROM automated_sweep_policies
            WHERE enabled = TRUE AND (next_sweep_at IS NULL OR next_sweep_at <= NOW())
            ORDER BY next_sweep_at ASC NULLS FIRST
            "#
        )
        .fetch_all(&self.pool)
        .await?)
    }
}
