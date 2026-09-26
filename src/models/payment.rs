use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Payment {
    pub id: Uuid,
    pub merchant_id: Uuid,
    pub wallet_id: Uuid,
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

#[derive(Debug, Clone)]
pub struct NewPayment {
    pub merchant_id: Uuid,
    pub wallet_id: Uuid,
    pub wallet_address: String,
    pub tx_hash: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub network: String,
}

#[derive(Debug, Clone, Copy)]
pub enum UpdatePaymentStatus {
    Verified,
    Confirmed,
    Failed,
}

/// Payment record kinds stored in the `payments` table.
///
/// Incoming merchant payments use [`PaymentKind::Payment`]. Funds swept from a
/// merchant custodial wallet to the platform settlement wallet
/// (`STELLAR_SYSTEM_WALLET_ADDRESS`) are recorded with [`PaymentKind::Sweep`].
pub const PAYMENT_KIND_PAYMENT: &str = "payment";
pub const PAYMENT_KIND_SWEEP: &str = "sweep";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentKind {
    Payment,
    Sweep,
}

impl PaymentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PaymentKind::Payment => PAYMENT_KIND_PAYMENT,
            PaymentKind::Sweep => PAYMENT_KIND_SWEEP,
        }
    }
}

impl std::fmt::Display for PaymentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A sweep transfer moving merchant funds to the platform settlement wallet.
///
/// The `tx_hash` is the Stellar transaction hash of the signed transfer and
/// `destination_address` is the settlement wallet the funds were moved to.
#[derive(Debug, Clone)]
pub struct NewSweepPayment {
    pub merchant_id: Uuid,
    pub wallet_id: Uuid,
    pub wallet_address: String,
    pub destination_address: String,
    pub tx_hash: String,
    pub amount_stroops: i64,
    pub asset: String,
    pub network: String,
}

impl NewSweepPayment {
    /// Builds the sweep record from a confirmed payment, preserving the source
    /// wallet and asset details while pointing at the settlement destination.
    pub fn from_confirmed_payment(
        payment: &Payment,
        destination_address: String,
        tx_hash: String,
    ) -> Self {
        Self {
            merchant_id: payment.merchant_id,
            wallet_id: payment.wallet_id,
            wallet_address: payment.wallet_address.clone(),
            destination_address,
            tx_hash,
            amount_stroops: payment.amount_stroops,
            asset: payment.asset.clone(),
            network: payment.network.clone(),
        }
    }

    pub fn kind(&self) -> PaymentKind {
        PaymentKind::Sweep
    }
}
