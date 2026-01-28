//! Payment provider trait definitions
//!
//! Defines the common interface that all payment providers must implement.

use crate::error::AppResult;
use crate::payments::types::{
    PaymentRequest, PaymentResponse, PaymentStatus, WithdrawalRequest, WithdrawalResponse,
};
use async_trait::async_trait;

/// Trait for payment provider implementations
///
/// All payment providers (Paystack, Flutterwave, M-Pesa) must implement this trait
/// to provide a unified interface for payment operations.
#[async_trait]
pub trait PaymentProvider: Send + Sync {
    /// Initialize a payment transaction
    ///
    /// This method initiates a payment with the provider, returning an authorization
    /// URL or embed code that the user can use to complete the payment.
    ///
    /// # Arguments
    /// * `request` - Payment request containing amount, currency, customer details, etc.
    ///
    /// # Returns
    /// * `PaymentResponse` - Contains authorization URL, reference, and other metadata
    async fn initiate_payment(&self, request: PaymentRequest) -> AppResult<PaymentResponse>;

    /// Verify the status of a payment transaction
    ///
    /// Checks the current status of a payment using the transaction reference.
    ///
    /// # Arguments
    /// * `reference` - Unique transaction reference returned from `initiate_payment`
    ///
    /// # Returns
    /// * `PaymentStatus` - Current status of the payment (success, pending, failed, etc.)
    async fn verify_payment(&self, reference: &str) -> AppResult<PaymentStatus>;

    /// Process a withdrawal/transfer to a bank account or mobile wallet
    ///
    /// This method handles the transfer of funds from the platform to a user's
    /// bank account or mobile wallet.
    ///
    /// # Arguments
    /// * `request` - Withdrawal request containing recipient details, amount, currency
    ///
    /// # Returns
    /// * `WithdrawalResponse` - Contains transfer reference and status
    async fn process_withdrawal(&self, request: WithdrawalRequest) -> AppResult<WithdrawalResponse>;

    /// Validate webhook signature
    ///
    /// Verifies that a webhook request is authentic and came from the payment provider.
    ///
    /// # Arguments
    /// * `payload` - Raw webhook payload body
    /// * `signature` - Signature from webhook header
    ///
    /// # Returns
    /// * `bool` - True if signature is valid, false otherwise
    fn validate_webhook_signature(&self, payload: &[u8], signature: &str) -> bool;
}
