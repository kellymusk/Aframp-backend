//! Payment provider types and data structures
//!
//! Common types used across all payment providers for requests and responses.

use serde::{Deserialize, Serialize};

/// Payment request for initiating a transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentRequest {
    /// Customer email address
    pub email: String,
    /// Amount in smallest currency unit (e.g., kobo for NGN, pesewas for GHS)
    pub amount: String,
    /// Currency code (NGN, GHS, ZAR, etc.)
    pub currency: String,
    /// Unique reference for this transaction (for idempotency)
    pub reference: String,
    /// Callback URL to redirect after payment
    pub callback_url: Option<String>,
    /// Payment channels to enable (card, bank, ussd, etc.)
    pub channels: Option<Vec<String>>,
    /// Additional metadata
    pub metadata: Option<serde_json::Value>,
}

/// Payment response from provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentResponse {
    /// Authorization URL for redirect-based payments
    pub authorization_url: Option<String>,
    /// Access code for inline payment forms
    pub access_code: Option<String>,
    /// Transaction reference
    pub reference: String,
    /// Provider-specific response data
    pub provider_data: Option<serde_json::Value>,
}

/// Payment status information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PaymentStatus {
    /// Payment was successful
    Success {
        amount: String,
        currency: String,
        paid_at: Option<String>,
        channel: Option<String>,
    },
    /// Payment is pending
    Pending,
    /// Payment failed
    Failed {
        reason: Option<String>,
    },
    /// Payment was reversed/refunded
    Reversed,
    /// Unknown status
    Unknown,
}

/// Withdrawal request for transferring funds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawalRequest {
    /// Recipient name
    pub recipient_name: String,
    /// Bank account number
    pub account_number: String,
    /// Bank code (provider-specific)
    pub bank_code: String,
    /// Amount in smallest currency unit
    pub amount: String,
    /// Currency code
    pub currency: String,
    /// Unique reference for this withdrawal
    pub reference: String,
    /// Reason/description for withdrawal
    pub reason: Option<String>,
    /// Additional metadata
    pub metadata: Option<serde_json::Value>,
}

/// Withdrawal response from provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawalResponse {
    /// Transfer reference
    pub transfer_reference: String,
    /// Transfer status
    pub status: WithdrawalStatus,
    /// Provider-specific response data
    pub provider_data: Option<serde_json::Value>,
}

/// Withdrawal status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WithdrawalStatus {
    /// Transfer is pending
    Pending,
    /// Transfer was successful
    Success,
    /// Transfer failed
    Failed {
        reason: Option<String>,
    },
    /// Transfer was reversed
    Reversed,
}
