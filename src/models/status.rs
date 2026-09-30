//! Compile-time-safe status enums that map 1:1 to the DB CHECK-constrained
//! `status` columns in the `withdrawals`, `payments`, and `payment_requests`
//! tables.  Using these in queries prevents a class of silent string-typo bugs.

use serde::{Deserialize, Serialize};

/// Status of a withdrawal request.
///
/// Corresponds to the `status` column in the `withdrawals` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum WithdrawalStatus {
    /// Withdrawal has been requested and debited from balance, payout not yet
    /// sent.
    Pending,
    /// Payout has been submitted to the provider and is in flight.
    Processing,
    /// Payout confirmed as successfully delivered.
    Completed,
    /// Payout failed; balance has been refunded.
    Failed,
}

impl WithdrawalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            WithdrawalStatus::Pending => "pending",
            WithdrawalStatus::Processing => "processing",
            WithdrawalStatus::Completed => "completed",
            WithdrawalStatus::Failed => "failed",
        }
    }
}

impl std::fmt::Display for WithdrawalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Status of an on-chain payment / deposit.
///
/// Corresponds to the `status` column in the `payments` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum PaymentStatus {
    /// Transaction seen on the network; not yet confirmed to depth.
    Detected,
    /// Passed the soft confirmation threshold.
    Verified,
    /// Passed the final confirmation threshold; funds are spendable.
    Confirmed,
    /// Transaction was rejected or rolled back.
    Failed,
}

impl PaymentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            PaymentStatus::Detected => "detected",
            PaymentStatus::Verified => "verified",
            PaymentStatus::Confirmed => "confirmed",
            PaymentStatus::Failed => "failed",
        }
    }
}

impl std::fmt::Display for PaymentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Status of a payment request.
///
/// Corresponds to the `status` column in the `payment_requests` table.
/// Note: `expired` is a virtual status computed at read time — a `pending`
/// request whose `expires_at` has passed is reported as expired without any
/// background job flipping the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum PaymentRequestStatus {
    /// Awaiting payment; may be logically expired if `expires_at` has passed.
    Pending,
    /// Payment received in full.
    Paid,
    /// Payment received but amount was less than requested.
    Partial,
}

impl PaymentRequestStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            PaymentRequestStatus::Pending => "pending",
            PaymentRequestStatus::Paid => "paid",
            PaymentRequestStatus::Partial => "partial",
        }
    }
}

impl std::fmt::Display for PaymentRequestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn withdrawal_status_serialises_correctly() {
        assert_eq!(WithdrawalStatus::Pending.as_str(), "pending");
        assert_eq!(WithdrawalStatus::Processing.as_str(), "processing");
        assert_eq!(WithdrawalStatus::Completed.as_str(), "completed");
        assert_eq!(WithdrawalStatus::Failed.as_str(), "failed");

        let json = serde_json::to_string(&WithdrawalStatus::Pending).unwrap();
        assert_eq!(json, r#""Pending""#);
    }

    #[test]
    fn payment_status_serialises_correctly() {
        assert_eq!(PaymentStatus::Detected.as_str(), "detected");
        assert_eq!(PaymentStatus::Verified.as_str(), "verified");
        assert_eq!(PaymentStatus::Confirmed.as_str(), "confirmed");
        assert_eq!(PaymentStatus::Failed.as_str(), "failed");

        let json = serde_json::to_string(&PaymentStatus::Detected).unwrap();
        assert_eq!(json, r#""Detected""#);
    }

    #[test]
    fn payment_request_status_serialises_correctly() {
        assert_eq!(PaymentRequestStatus::Pending.as_str(), "pending");
        assert_eq!(PaymentRequestStatus::Paid.as_str(), "paid");
        assert_eq!(PaymentRequestStatus::Partial.as_str(), "partial");

        let json = serde_json::to_string(&PaymentRequestStatus::Pending).unwrap();
        assert_eq!(json, r#""Pending""#);
    }

    #[test]
    fn display_impl_matches_as_str() {
        assert_eq!(
            format!("{}", WithdrawalStatus::Completed),
            WithdrawalStatus::Completed.as_str()
        );
        assert_eq!(
            format!("{}", PaymentStatus::Confirmed),
            PaymentStatus::Confirmed.as_str()
        );
        assert_eq!(
            format!("{}", PaymentRequestStatus::Paid),
            PaymentRequestStatus::Paid.as_str()
        );
    }
}
