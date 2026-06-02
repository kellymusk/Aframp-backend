//! Treasury bonds service — bond registration, allocation, sweep, and liquidation

use chrono::Utc;
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;

use super::{
    models::{
        AllocateBondRequest, AutomatedSweepPolicy, BondLedgerAllocation, RegisterBondRequest,
        SweepPolicyRequest, TokenizedBondInstrument,
    },
    repository::TreasuryBondsRepository,
};

pub struct TreasuryBondsService {
    repo: TreasuryBondsRepository,
}

impl TreasuryBondsService {
    pub fn new(pool: PgPool) -> Self {
        Self { repo: TreasuryBondsRepository::new(pool) }
    }

    /// Register a new government bond instrument.
    pub async fn register_instrument(
        &self,
        req: RegisterBondRequest,
    ) -> Result<TokenizedBondInstrument, anyhow::Error> {
        let face_value: sqlx::types::BigDecimal = req.face_value.parse()?;
        let now = Utc::now();

        let inst = TokenizedBondInstrument {
            id: Uuid::new_v4(),
            isin: req.isin.clone(),
            issuer_authority: req.issuer_authority.clone(),
            instrument_name: req.instrument_name.clone(),
            currency: req.currency.unwrap_or_else(|| "NGN".into()),
            face_value,
            coupon_rate_bps: req.coupon_rate_bps,
            maturity_at: req.maturity_at,
            auction_date: req.auction_date,
            on_chain_asset_code: None,
            stellar_issuer: None,
            status: "ACTIVE".into(),
            metadata: serde_json::Value::Object(Default::default()),
            created_at: now,
            updated_at: now,
        };

        let saved = self.repo.create_instrument(&inst).await?;
        info!(isin = %req.isin, instrument_id = %saved.id, "Bond instrument registered");
        Ok(saved)
    }

    pub async fn get_instrument(
        &self,
        id: Uuid,
    ) -> Result<Option<TokenizedBondInstrument>, anyhow::Error> {
        self.repo.get_instrument(id).await
    }

    pub async fn list_instruments(&self) -> Result<Vec<TokenizedBondInstrument>, anyhow::Error> {
        self.repo.list_active_instruments().await
    }

    /// Allocate fractional bond units to a tenant (simulates on-chain minting).
    pub async fn allocate(
        &self,
        req: AllocateBondRequest,
    ) -> Result<BondLedgerAllocation, anyhow::Error> {
        let fractional_units: sqlx::types::BigDecimal = req.fractional_units.parse()?;
        let purchase_price: sqlx::types::BigDecimal = req.purchase_price.parse()?;
        let now = Utc::now();

        let alloc = BondLedgerAllocation {
            id: Uuid::new_v4(),
            tenant_id: req.tenant_id,
            bond_instrument_id: req.bond_instrument_id,
            fractional_units,
            purchase_price,
            accrued_yield: "0".parse()?,
            on_chain_token_hash: Some(format!("0x{}", Uuid::new_v4().simple())),
            stellar_tx_hash: None,
            status: "ACTIVE".into(),
            acquired_at: now,
            redeemed_at: None,
            created_at: now,
            updated_at: now,
        };

        let saved = self.repo.create_allocation(&alloc).await?;
        info!(
            tenant_id = %req.tenant_id,
            instrument_id = %req.bond_instrument_id,
            allocation_id = %saved.id,
            "Bond allocation created"
        );
        Ok(saved)
    }

    pub async fn get_allocation(
        &self,
        id: Uuid,
    ) -> Result<Option<BondLedgerAllocation>, anyhow::Error> {
        self.repo.get_allocation(id).await
    }

    pub async fn list_tenant_allocations(
        &self,
        tenant_id: Uuid,
    ) -> Result<Vec<BondLedgerAllocation>, anyhow::Error> {
        self.repo.list_tenant_allocations(tenant_id).await
    }

    /// Emergency fractional liquidation — converts bond allocation back to liquid fiat.
    pub async fn liquidate(&self, allocation_id: Uuid) -> Result<BondLedgerAllocation, anyhow::Error> {
        let updated = self.repo.liquidate_allocation(allocation_id).await?;
        info!(allocation_id = %allocation_id, "Bond allocation liquidated");
        Ok(updated)
    }

    /// Upsert automated sweep policy for a tenant.
    pub async fn upsert_sweep_policy(
        &self,
        req: SweepPolicyRequest,
    ) -> Result<AutomatedSweepPolicy, anyhow::Error> {
        let threshold: sqlx::types::BigDecimal = req
            .min_sweep_threshold_ngn
            .as_deref()
            .unwrap_or("1000000")
            .parse()?;
        let now = Utc::now();
        let interval = req.sweep_interval_minutes.unwrap_or(60);
        let next_sweep = now + chrono::Duration::minutes(interval as i64);

        let policy = AutomatedSweepPolicy {
            id: Uuid::new_v4(),
            tenant_id: req.tenant_id,
            enabled: req.enabled.unwrap_or(true),
            min_sweep_threshold_ngn: threshold,
            max_portfolio_duration_days: req.max_portfolio_duration_days.unwrap_or(90),
            preferred_instrument_id: req.preferred_instrument_id,
            last_sweep_at: None,
            next_sweep_at: Some(next_sweep),
            sweep_interval_minutes: interval,
            created_at: now,
            updated_at: now,
        };

        let saved = self.repo.upsert_sweep_policy(&policy).await?;
        info!(tenant_id = %req.tenant_id, "Sweep policy upserted");
        Ok(saved)
    }

    pub async fn get_sweep_policy(
        &self,
        tenant_id: Uuid,
    ) -> Result<Option<AutomatedSweepPolicy>, anyhow::Error> {
        self.repo.get_sweep_policy(tenant_id).await
    }

    /// Called by the background sweep daemon to list tenants due for a sweep.
    pub async fn list_due_sweeps(&self) -> Result<Vec<AutomatedSweepPolicy>, anyhow::Error> {
        self.repo.list_due_sweeps().await
    }
}
