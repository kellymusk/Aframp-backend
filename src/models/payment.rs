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

impl Payment {
    /// Header row for the CSV export of transaction history.
    pub const CSV_HEADER: &'static str = "id,merchant_id,wallet_id,wallet_address,tx_hash,amount_stroops,asset,network,status,confirmations,created_at,updated_at";

    /// Render this payment as a single CSV record (no trailing newline).
    ///
    /// Fields are escaped per RFC 4180 so values containing commas, quotes,
    /// or newlines do not corrupt the exported file.
    pub fn to_csv_record(&self) -> String {
        let fields = [
            self.id.to_string(),
            self.merchant_id.to_string(),
            self.wallet_id.to_string(),
            csv_escape(&self.wallet_address),
            csv_escape(&self.tx_hash),
            self.amount_stroops.to_string(),
            csv_escape(&self.asset),
            csv_escape(&self.network),
            csv_escape(&self.status),
            self.confirmations.to_string(),
            self.created_at.to_rfc3339(),
            self.updated_at.to_rfc3339(),
        ];
        fields.join(",")
    }
}

/// Escape a single CSV field per RFC 4180: wrap in double quotes when the
/// value contains a comma, double quote, CR, or LF, and double any embedded
/// quotes.
fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
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