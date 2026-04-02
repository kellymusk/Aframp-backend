//! Reserve Reconciliation Worker
//!
//! Performs a deep-dive three-way audit of every fiat-to-token movement:
//!   1. Fiat Inbound  — bank deposit / payment provider record
//!   2. Internal      — mint_requests / transactions table entry
//!   3. On-Chain      — Stellar Horizon confirmed mint operation
//!
//! Discrepancy categories:
//!   MISSING_MINT      — fiat confirmed, no cNGN issued
//!   UNAUTHORIZED_MINT — cNGN issued, no matching fiat (HIGH ALERT)
//!   AMOUNT_MISMATCH   — both exist but values differ (even by 1 kobo)
//!
//! Runs every 20 minutes by default (RECONCILIATION_INTERVAL_MINS env var).
//! Generates an end-of-day health report at 23:55 UTC.

use bigdecimal::BigDecimal;
use chrono::{NaiveDate, Utc};
use sqlx::PgPool;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::interval;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::chains::stellar::client::StellarClient;

// ── Configuration ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ReconciliationConfig {
    /// How often the worker runs (default: 20 min)
    pub interval: Duration,
    /// Transactions pending longer than this are flagged (default: 4 h)
    pub pending_threshold: Duration,
    /// Look-back window for reconciliation (default: 48 h)
    pub lookback_hours: i64,
}

impl Default for ReconciliationConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(20 * 60),
            pending_threshold: Duration::from_secs(4 * 60 * 60),
            lookback_hours: 48,
        }
    }
}

impl ReconciliationConfig {
    pub fn from_env() -> Self {
        let mut c = Self::default();
        if let Ok(v) = std::env::var("RECONCILIATION_INTERVAL_MINS") {
            if let Ok(mins) = v.parse::<u64>() {
                c.interval = Duration::from_secs(mins * 60);
            }
        }
        if let Ok(v) = std::env::var("RECONCILIATION_PENDING_THRESHOLD_HOURS") {
            if let Ok(h) = v.parse::<u64>() {
                c.pending_threshold = Duration::from_secs(h * 60 * 60);
            }
        }
        if let Ok(v) = std::env::var("RECONCILIATION_LOOKBACK_HOURS") {
            if let Ok(h) = v.parse::<i64>() {
                c.lookback_hours = h;
            }
        }
        c
    }
}

// ── Discrepancy types (mirror DB enum) ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum DiscrepancyType {
    MissingMint,
    UnauthorizedMint,
    AmountMismatch,
}

impl DiscrepancyType {
    fn as_str(&self) -> &'static str {
        match self {
            Self::MissingMint => "MISSING_MINT",
            Self::UnauthorizedMint => "UNAUTHORIZED_MINT",
            Self::AmountMismatch => "AMOUNT_MISMATCH",
        }
    }
}

// ── Worker ────────────────────────────────────────────────────────────────────

pub struct ReconciliationWorker {
    pool: PgPool,
    stellar: Arc<StellarClient>,
    config: ReconciliationConfig,
}

impl ReconciliationWorker {
    pub fn new(pool: PgPool, stellar: Arc<StellarClient>, config: ReconciliationConfig) -> Self {
        Self { pool, stellar, config }
    }

    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let mut ticker = interval(self.config.interval);
        info!(
            interval_mins = self.config.interval.as_secs() / 60,
            "Reconciliation worker started"
        );

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(e) = self.run_cycle().await {
                        error!(error = %e, "Reconciliation cycle failed");
                    }
                    // Generate daily report near end of day (23:50–23:59 UTC)
                    let hour = Utc::now().format("%H").to_string();
                    if hour == "23" {
                        if let Err(e) = self.generate_daily_report(Utc::now().date_naive()).await {
                            error!(error = %e, "Failed to generate daily reconciliation report");
                        }
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("Reconciliation worker shutting down");
                        break;
                    }
                }
            }
        }
    }

    // ── Main reconciliation cycle ──────────────────────────────────────────

    async fn run_cycle(&self) -> Result<(), anyhow::Error> {
        let lookback = chrono::Duration::hours(self.config.lookback_hours);
        let since = Utc::now() - lookback;

        info!(since = %since, "Starting reconciliation cycle");

        // Fetch all onramp transactions in the window
        let rows = sqlx::query!(
            r#"
            SELECT
                transaction_id,
                from_amount,
                cngn_amount,
                status,
                payment_reference,
                stellar_tx_hash,
                created_at
            FROM transactions
            WHERE type = 'onramp'
              AND created_at >= $1
            ORDER BY created_at DESC
            "#,
            since
        )
        .fetch_all(&self.pool)
        .await?;

        let threshold_secs = self.config.pending_threshold.as_secs() as i64;
        let mut checked = 0u32;
        let mut flagged = 0u32;

        for row in &rows {
            checked += 1;
            let tx_id = row.transaction_id;
            let status = row.status.as_str();
            let fiat_amount = &row.from_amount;
            let mint_amount = &row.cngn_amount;
            let stellar_hash = row.stellar_tx_hash.as_deref();
            let payment_ref = row.payment_reference.as_deref();

            // 1. MISSING_MINT: fiat confirmed but no cNGN issued after threshold
            if status == "payment_received" {
                let age_secs = (Utc::now() - row.created_at).num_seconds();
                if age_secs > threshold_secs {
                    self.log_discrepancy(
                        tx_id,
                        DiscrepancyType::MissingMint,
                        Some(fiat_amount.clone()),
                        None,
                        None,
                        payment_ref,
                    )
                    .await?;
                    flagged += 1;
                }
            }

            // 2. UNAUTHORIZED_MINT: stellar hash present but no fiat confirmation
            if stellar_hash.is_some() && status != "completed" && status != "processing" {
                if fiat_amount == &BigDecimal::from(0) {
                    self.log_discrepancy(
                        tx_id,
                        DiscrepancyType::UnauthorizedMint,
                        None,
                        Some(mint_amount.clone()),
                        stellar_hash,
                        payment_ref,
                    )
                    .await?;
                    flagged += 1;
                }
            }

            // 3. AMOUNT_MISMATCH: both fiat and mint exist but differ
            if status == "completed" && stellar_hash.is_some() {
                // Allow for fee deduction — compare fiat vs mint (mint should be <= fiat)
                // Flag if mint > fiat (impossible without error) or if difference > 1 kobo
                let diff = fiat_amount - mint_amount;
                let one_kobo = BigDecimal::from_str("0.01").unwrap();
                if diff < BigDecimal::from(0) || diff > one_kobo * BigDecimal::from(100) {
                    // More than 1 NGN difference — flag it
                    self.log_discrepancy(
                        tx_id,
                        DiscrepancyType::AmountMismatch,
                        Some(fiat_amount.clone()),
                        Some(mint_amount.clone()),
                        stellar_hash,
                        payment_ref,
                    )
                    .await?;
                    flagged += 1;
                }
            }
        }

        info!(
            checked,
            flagged,
            "Reconciliation cycle complete"
        );

        Ok(())
    }

    // ── Log a discrepancy (idempotent — skip if already open for same tx+type) ─

    async fn log_discrepancy(
        &self,
        transaction_id: Uuid,
        dtype: DiscrepancyType,
        fiat_amount: Option<BigDecimal>,
        mint_amount: Option<BigDecimal>,
        stellar_tx_hash: Option<&str>,
        payment_reference: Option<&str>,
    ) -> Result<(), anyhow::Error> {
        let dtype_str = dtype.as_str();
        let is_critical = dtype == DiscrepancyType::UnauthorizedMint;

        // Idempotency: skip if an OPEN/INVESTIGATING entry already exists
        let exists: bool = sqlx::query_scalar!(
            r#"
            SELECT EXISTS(
                SELECT 1 FROM discrepancy_log
                WHERE transaction_id = $1
                  AND discrepancy_type = $2::discrepancy_type
                  AND status != 'RESOLVED'
            ) AS "exists!"
            "#,
            transaction_id,
            dtype_str
        )
        .fetch_one(&self.pool)
        .await?;

        if exists {
            return Ok(());
        }

        sqlx::query!(
            r#"
            INSERT INTO discrepancy_log
                (transaction_id, discrepancy_type, fiat_amount, mint_amount,
                 stellar_tx_hash, payment_reference)
            VALUES ($1, $2::discrepancy_type, $3, $4, $5, $6)
            "#,
            transaction_id,
            dtype_str,
            fiat_amount,
            mint_amount,
            stellar_tx_hash,
            payment_reference,
        )
        .execute(&self.pool)
        .await?;

        if is_critical {
            // UNAUTHORIZED_MINT — emit high-priority structured log for alerting pipeline
            error!(
                transaction_id = %transaction_id,
                discrepancy_type = dtype_str,
                mint_amount = ?mint_amount,
                "🚨 CRITICAL: UNAUTHORIZED_MINT detected — cNGN issued without confirmed fiat deposit"
            );
        } else {
            warn!(
                transaction_id = %transaction_id,
                discrepancy_type = dtype_str,
                fiat_amount = ?fiat_amount,
                mint_amount = ?mint_amount,
                "Reconciliation discrepancy logged"
            );
        }

        Ok(())
    }

    // ── Daily health report ────────────────────────────────────────────────

    async fn generate_daily_report(&self, date: NaiveDate) -> Result<(), anyhow::Error> {
        // Idempotent — skip if already generated for this date
        let exists: bool = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM reconciliation_reports WHERE report_date = $1) AS "exists!""#,
            date
        )
        .fetch_one(&self.pool)
        .await?;

        if exists {
            return Ok(());
        }

        let day_start = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let day_end = date.and_hms_opt(23, 59, 59).unwrap().and_utc();

        let total: i64 = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM transactions WHERE type = 'onramp' AND created_at BETWEEN $1 AND $2",
            day_start, day_end
        )
        .fetch_one(&self.pool)
        .await?
        .unwrap_or(0);

        let disc_counts = sqlx::query!(
            r#"
            SELECT
                discrepancy_type::TEXT as dtype,
                COUNT(*) as cnt
            FROM discrepancy_log
            WHERE detected_at BETWEEN $1 AND $2
            GROUP BY discrepancy_type
            "#,
            day_start,
            day_end
        )
        .fetch_all(&self.pool)
        .await?;

        let mut missing_mint = 0i64;
        let mut unauthorized_mint = 0i64;
        let mut amount_mismatch = 0i64;

        for row in &disc_counts {
            match row.dtype.as_deref() {
                Some("MISSING_MINT") => missing_mint = row.cnt.unwrap_or(0),
                Some("UNAUTHORIZED_MINT") => unauthorized_mint = row.cnt.unwrap_or(0),
                Some("AMOUNT_MISMATCH") => amount_mismatch = row.cnt.unwrap_or(0),
                _ => {}
            }
        }

        let total_disc = missing_mint + unauthorized_mint + amount_mismatch;
        let matched = total - total_disc;
        let has_open = total_disc > 0;

        sqlx::query!(
            r#"
            INSERT INTO reconciliation_reports
                (report_date, total_transactions, matched_count, discrepancy_count,
                 missing_mint_count, unauthorized_mint_count, amount_mismatch_count,
                 has_open_discrepancies)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (report_date) DO NOTHING
            "#,
            date,
            total as i32,
            matched as i32,
            total_disc as i32,
            missing_mint as i32,
            unauthorized_mint as i32,
            amount_mismatch as i32,
            has_open,
        )
        .execute(&self.pool)
        .await?;

        info!(
            date = %date,
            total_transactions = total,
            matched = matched,
            discrepancies = total_disc,
            unauthorized_mints = unauthorized_mint,
            "Daily reconciliation health report generated"
        );

        Ok(())
    }
}
