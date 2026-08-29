//! Offramp API endpoints for cNGN withdrawal transactions
//!
//! This module handles the POST /api/offramp/initiate endpoint that:
//! - Validates withdrawal quotes
//! - Verifies bank account details with payment providers
//! - Creates pending withdrawal transactions
//! - Provides payment instructions for sending cNGN to system wallet

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use bigdecimal::BigDecimal;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::cache::cache::Cache;
use crate::cache::keys::onramp::QuoteKey;
use crate::cache::RedisCache;
use crate::database::transaction_repository::TransactionRepository;
use crate::error::{AppError, AppErrorKind, DomainError, ValidationError};
use crate::payments::factory::PaymentProviderFactory;
use crate::security::{AnomalyDetectionService, CircuitBreakerMiddleware};
use crate::services::bank_verification::BankVerificationService;
use crate::services::onramp_quote::StoredQuote;
use sqlx::PgPool;

// ===== REQUEST/RESPONSE TYPES =====

/// Offramp initiate request
#[derive(Debug, Clone, Deserialize)]
pub struct OfframpInitiateRequest {
    pub quote_id: String,
    pub wallet_address: String,
    pub bank_details: BankDetails,
}

/// Bank account details for withdrawal
#[derive(Debug, Clone, Deserialize)]
pub struct BankDetails {
    pub bank_code: String,      // 3-digit Nigerian bank code
    pub account_number: String, // 10-digit account number
    pub account_name: String,   // Account holder name
}

/// Bank account verification result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedBankDetails {
    pub bank_code: String,
    pub account_number: String,
    pub account_name: String,
    pub bank_name: Option<String>,
}

/// Payment instructions for cNGN transfer
#[derive(Debug, Clone, Serialize)]
pub struct PaymentInstructions {
    pub send_to_address: String,
    pub send_amount: String,
    pub send_asset: String,
    pub asset_issuer: String,
    pub memo_text: String,
    pub memo_type: String,
    pub memo_required: bool,
}

/// Requirements info
#[derive(Debug, Clone, Serialize)]
pub struct RequirementsInfo {
    pub min_xlm_for_fees: String,
    pub exact_amount_required: bool,
    pub memo_required: bool,
}

/// Transaction timeline
#[derive(Debug, Clone, Serialize)]
pub struct Timeline {
    pub send_payment_by: String,
    pub expected_confirmation: String,
    pub expected_withdrawal: String,
    pub expires_at: String,
}

/// Withdrawal details
#[derive(Debug, Clone, Serialize)]
pub struct WithdrawalDetailsInfo {
    pub bank_name: Option<String>,
    pub account_number: String,
    pub account_name: String,
    pub amount_to_receive: String,
}

/// Quote info in response
#[derive(Debug, Clone, Serialize)]
pub struct QuoteInfo {
    pub cngn_amount: String,
    pub ngn_amount: String,
    pub total_fees: String,
}

/// Success response for initiate endpoint
#[derive(Debug, Serialize)]
pub struct OfframpInitiateResponse {
    pub transaction_id: String,
    pub status: String,
    pub quote: QuoteInfo,
    pub payment_instructions: PaymentInstructions,
    pub requirements: RequirementsInfo,
    pub withdrawal_details: WithdrawalDetailsInfo,
    pub timeline: Timeline,
    pub next_steps: Vec<String>,
    pub created_at: String,
}

/// Error response detail
#[derive(Debug, Serialize)]
pub struct ErrorResponseDetail {
    pub code: String,
    pub message: String,
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bank_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provided_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_name: Option<String>,
}

/// Error response wrapper
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorResponseDetail,
}

/// Offramp state for dependency injection
#[derive(Clone)]
pub struct OfframpState {
    pub db_pool: Arc<PgPool>,
    pub redis_cache: Arc<RedisCache>,
    pub payment_provider_factory: Arc<PaymentProviderFactory>,
    pub bank_verification_service: Arc<BankVerificationService>,
    pub system_wallet_address: String,
    pub cngn_issuer_address: String,
    pub circuit_breaker: Arc<CircuitBreakerMiddleware>,
}

// ===== CONSTANTS =====

/// Quote expiration time in seconds (5 minutes)
const QUOTE_EXPIRY_SECS: i64 = 300;

/// Withdrawal expiration time in seconds (30 minutes)
const WITHDRAWAL_EXPIRY_SECS: i64 = 1800;

/// Minimum XLM for transaction fees
const MIN_XLM_FOR_FEES: &str = "0.01";

/// Maximum amount to help detect memo collisions (extremely unlikely but included for safety)
const MAX_MEMO_COLLISION_ATTEMPTS: u32 = 3;

// ===== SUPPORTED BANKS =====

/// Supported Nigerian banks mapping
fn get_supported_banks() -> std::collections::HashMap<String, String> {
    let mut banks = std::collections::HashMap::new();
    banks.insert("044".to_string(), "Access Bank".to_string());
    banks.insert("058".to_string(), "GTBank".to_string());
    banks.insert("033".to_string(), "United Bank for Africa".to_string());
    banks.insert("032".to_string(), "Union Bank".to_string());
    banks.insert("011".to_string(), "First Bank".to_string());
    banks.insert("057".to_string(), "Zenith Bank".to_string());
    banks.insert("050".to_string(), "Ecobank".to_string());
    banks.insert("035".to_string(), "Wema Bank".to_string());
    banks.insert("051".to_string(), "Fidelity Bank".to_string());
    banks.insert(
        "053".to_string(),
        "Bank of the Philippine Islands".to_string(),
    );
    banks.insert("052".to_string(), "Stanbic IBTC".to_string());
    banks.insert("021".to_string(), "Polaris Bank".to_string());
    banks.insert("048".to_string(), "Citi Bank".to_string());
    banks.insert("101".to_string(), "Providus Bank".to_string());
    banks.insert("100".to_string(), "Sycamore Bank".to_string());
    banks.insert("102".to_string(), "Titan Trust Bank".to_string());
    banks
}

// ===== VALIDATION FUNCTIONS =====

/// Validate bank code exists in supported banks list
fn validate_bank_code(bank_code: &str) -> Result<String, AppError> {
    let banks = get_supported_banks();
    banks.get(bank_code).cloned().ok_or_else(|| {
        AppError::new(AppErrorKind::Validation(ValidationError::InvalidAmount {
            amount: bank_code.to_string(),
            reason: format!(
                "Bank code '{}' is not supported. Supported codes: {}",
                bank_code,
                banks.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        }))
    })
}

/// Validate account number format (must be 10 digits)
fn validate_account_number(account_number: &str) -> Result<(), AppError> {
    let trimmed = account_number.trim();
    if trimmed.len() != 10 || !trimmed.chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::new(AppErrorKind::Validation(
            ValidationError::InvalidAmount {
                amount: account_number.to_string(),
                reason: "Account number must be exactly 10 digits".to_string(),
            },
        )));
    }
    Ok(())
}

/// Validate account name (not empty, reasonable length)
fn validate_account_name(account_name: &str) -> Result<(), AppError> {
    let trimmed = account_name.trim();
    if trimmed.is_empty() {
        return Err(AppError::new(AppErrorKind::Validation(
            ValidationError::MissingField {
                field: "account_name".to_string(),
            },
        )));
    }
    if trimmed.len() > 200 {
        return Err(AppError::new(AppErrorKind::Validation(
            ValidationError::InvalidAmount {
                amount: account_name.to_string(),
                reason: "Account name is too long (max 200 characters)".to_string(),
            },
        )));
    }
    Ok(())
}

/// Generate a unique withdrawal memo from transaction ID
///
/// Purpose:
/// - Links incoming cNGN to specific withdrawal transaction
/// - Unique identifier for transaction matching via Stellar memo field
/// - Required for payment tracking and reconciliation
///
/// Format:
/// - Format: WD-{first_8_chars_of_uuid}
/// - Example: WD-9F8E7D6C
/// - Length: 11 characters (well within Stellar 28-byte text memo limit)
///
/// Properties:
/// - Unique per transaction (based on UUID)
/// - Short and easy for users to copy
/// - Uppercase alphanumeric (easy to read in Stellar wallets)
/// - Stellar text memo compatible
fn generate_withdrawal_memo(transaction_id: &Uuid) -> String {
    let id_str = transaction_id.simple().to_string();
    let short_id = &id_str[..8];
    format!("WD-{}", short_id.to_uppercase())
}

// ===== QUOTE VALIDATION =====

/// Validate quote from Redis
async fn validate_quote(
    redis_cache: &RedisCache,
    quote_id: &str,
    wallet_address: &str,
) -> Result<StoredQuote, AppError> {
    let cache_key = QuoteKey::new(quote_id).to_string();

    // Fetch quote from cache with explicit type annotation
    let cached_result: Result<Option<StoredQuote>, _> = redis_cache.get(&cache_key).await;

    let stored_quote = cached_result
        .map_err(|e| {
            error!(quote_id = %quote_id, error = %e, "Failed to fetch quote from Redis");
            AppError::new(AppErrorKind::Infrastructure(
                crate::error::InfrastructureError::Cache {
                    message: format!("Failed to retrieve quote: {}", e),
                },
            ))
        })?
        .ok_or_else(|| {
            info!(quote_id = %quote_id, "Quote not found");
            AppError::new(AppErrorKind::Validation(ValidationError::InvalidAmount {
                amount: quote_id.to_string(),
                reason: "Invalid quote ID. Please generate a new quote.".to_string(),
            }))
        })?;

    // Check if quote has expired
    if let Ok(expires_dt) = chrono::DateTime::parse_from_rfc3339(&stored_quote.expires_at) {
        let expires_utc = expires_dt.with_timezone(&Utc);
        if expires_utc < Utc::now() {
            info!(quote_id = %quote_id, "Quote has expired");
            return Err(AppError::new(AppErrorKind::Domain(
                DomainError::RateExpired {
                    quote_id: quote_id.to_string(),
                },
            )));
        }
    }

    // Check if quote status is pending (not already used)
    if stored_quote.status != "pending" {
        info!(quote_id = %quote_id, status = %stored_quote.status, "Quote already used");
        return Err(AppError::new(AppErrorKind::Validation(
            ValidationError::InvalidAmount {
                amount: quote_id.to_string(),
                reason: "This quote has already been used for a withdrawal".to_string(),
            },
        )));
    }

    // Verify wallet address matches
    if stored_quote.wallet_address != wallet_address {
        warn!(
            quote_id = %quote_id,
            expected_wallet = %stored_quote.wallet_address,
            provided_wallet = %wallet_address,
            "Wallet address mismatch"
        );
        return Err(AppError::new(AppErrorKind::Validation(
            ValidationError::InvalidWalletAddress {
                address: wallet_address.to_string(),
                reason: "Quote was generated for a different wallet.".to_string(),
            },
        )));
    }

    debug!(quote_id = %quote_id, "Quote validated successfully");
    Ok(stored_quote)
}

// ===== BANK ACCOUNT VERIFICATION =====

/// Verify bank account with payment provider
async fn verify_bank_account(
    service: &BankVerificationService,
    bank_code: &str,
    account_number: &str,
    account_name: &str,
) -> Result<VerifiedBankDetails, AppError> {
    // Validate format first
    validate_account_number(account_number)?;
    let bank_name = validate_bank_code(bank_code)?;
    validate_account_name(account_name)?;

    // Call payment provider API to verify account
    let verification_result = service
        .verify_account(bank_code, account_number, account_name)
        .await?;

    info!(
        bank_code = %bank_code,
        account_number = %account_number,
        verified_name = %verification_result.account_name,
        "Bank account verified successfully"
    );

    Ok(VerifiedBankDetails {
        bank_code: bank_code.to_string(),
        account_number: verification_result.account_number,
        account_name: verification_result.account_name,
        bank_name: Some(bank_name),
    })
}

// ===== TRANSACTION CREATION =====

/// Create withdrawal transaction in database
async fn create_withdrawal_transaction(
    db_pool: &PgPool,
    wallet_address: &str,
    quote: &StoredQuote,
    bank_details: &VerifiedBankDetails,
    memo: &str,
    expires_at: chrono::DateTime<Utc>,
) -> Result<(String, String), AppError> {
    let tx_repo = TransactionRepository::new(db_pool.clone());

    let metadata = json!({
        "quote_id": quote.quote_id,
        "payment_memo": memo,
        "bank_code": bank_details.bank_code,
        "account_number": bank_details.account_number,
        "account_name": bank_details.account_name,
        "bank_name": bank_details.bank_name,
        "withdrawal_type": "offramp",
        "expires_at": expires_at.to_rfc3339(),
    });

    let cngn_amount =
        BigDecimal::from_str(&quote.amount_cngn).unwrap_or_else(|_| BigDecimal::from(0));
    let ngn_amount_parsed = quote.amount_ngn;

    let tx = tx_repo
        .create_transaction(
            wallet_address,
            "offramp",
            "cNGN",
            "NGN",
            cngn_amount.clone(),
            BigDecimal::from(ngn_amount_parsed),
            cngn_amount,
            "pending_payment",
            None,
            Some(memo),
            metadata,
        )
        .await
        .map_err(|e| {
            error!(error = ?e, "Failed to create withdrawal transaction");
            AppError::new(AppErrorKind::Infrastructure(
                crate::error::InfrastructureError::Database {
                    message: format!("Failed to create transaction: {}", e),
                    is_retryable: false,
                },
            ))
        })?;

    let tx_id = tx.transaction_id.to_string();
    info!(transaction_id = %tx_id, "Withdrawal transaction created");

    Ok((tx_id, memo.to_string()))
}

// ===== MAIN HANDLER =====

/// POST /api/offramp/initiate
///
/// Initiates a cNGN withdrawal transaction by:
/// 1. Validating the quote is still valid
/// 2. Verifying bank account details
/// 3. Generating a unique payment memo
/// 4. Creating a pending withdrawal transaction
/// 5. Returning system wallet address and payment instructions
pub async fn initiate_withdrawal(
    State(state): State<Arc<OfframpState>>,
    Json(request): Json<OfframpInitiateRequest>,
) -> Response {
    // Check circuit breaker status before proceeding
    if let Err(e) = state.circuit_breaker.check_operation_allowed().await {
        return create_error_response(
            503,
            "SYSTEM_HALTED",
            "System temporarily unavailable due to security incident".to_string(),
            None,
        );
    }

    info!(
        quote_id = %request.quote_id,
        wallet = %request.wallet_address,
        "Processing offramp initiate request"
    );

    // 1. Validate quote
    let quote = match validate_quote(
        &state.redis_cache,
        &request.quote_id,
        &request.wallet_address,
    )
    .await
    {
        Ok(q) => q,
        Err(e) => {
            return handle_offramp_error(e);
        }
    };

    // 2. Verify bank details
    let verified_bank = match verify_bank_account(
        &state.bank_verification_service,
        &request.bank_details.bank_code,
        &request.bank_details.account_number,
        &request.bank_details.account_name,
    )
    .await
    {
        Ok(b) => b,
        Err(e) => {
            return handle_offramp_error(e);
        }
    };

    // 3. Generate unique memo
    let transaction_id = Uuid::new_v4();
    let memo = generate_withdrawal_memo(&transaction_id);

    // 4. Create transaction in database
    let expires_at = Utc::now() + chrono::Duration::seconds(WITHDRAWAL_EXPIRY_SECS);

    let (tx_id, _) = match create_withdrawal_transaction(
        &state.db_pool,
        &request.wallet_address,
        &quote,
        &verified_bank,
        &memo,
        expires_at,
    )
    .await
    {
        Ok((id, m)) => (id, m),
        Err(e) => {
            return handle_offramp_error(e);
        }
    };

    // 5. Mark quote as used in Redis
    let cache_key = QuoteKey::new(&quote.quote_id).to_string();
    let mut updated_quote = quote.clone();
    updated_quote.status = "consumed".to_string();

    if let Err(e) = state
        .redis_cache
        .set(
            &cache_key,
            &updated_quote,
            Some(std::time::Duration::from_secs(300)),
        )
        .await
    {
        error!(error = %e, "Failed to update quote status in Redis");
        // Continue anyway, as the transaction is already created
    }

    // 6. Format response
    let now = Utc::now();
    let send_by = expires_at;
    let response = OfframpInitiateResponse {
        transaction_id: tx_id.clone(),
        status: "pending_payment".to_string(),
        quote: QuoteInfo {
            cngn_amount: quote.amount_cngn.clone(),
            ngn_amount: format!("{}", quote.amount_ngn),
            total_fees: quote.total_fee_ngn.clone(),
        },
        payment_instructions: PaymentInstructions {
            send_to_address: state.system_wallet_address.clone(),
            send_amount: quote.amount_cngn.clone(),
            send_asset: "cNGN".to_string(),
            asset_issuer: state.cngn_issuer_address.clone(),
            memo_text: memo.clone(),
            memo_type: "text".to_string(),
            memo_required: true,
        },
        requirements: RequirementsInfo {
            min_xlm_for_fees: MIN_XLM_FOR_FEES.to_string(),
            exact_amount_required: true,
            memo_required: true,
        },
        withdrawal_details: WithdrawalDetailsInfo {
            bank_name: verified_bank.bank_name.clone(),
            account_number: verified_bank.account_number.clone(),
            account_name: verified_bank.account_name.clone(),
            amount_to_receive: format!("{} NGN", quote.amount_ngn),
        },
        timeline: Timeline {
            send_payment_by: send_by.to_rfc3339(),
            expected_confirmation: "5-10 seconds".to_string(),
            expected_withdrawal: "2-5 minutes after confirmation".to_string(),
            expires_at: expires_at.to_rfc3339(),
        },
        next_steps: vec![
            "Open your Stellar wallet (Freighter, Lobstr, etc.)".to_string(),
            format!("Send exactly {} cNGN", quote.amount_cngn),
            format!("To address: {}", state.system_wallet_address),
            format!("Include memo: {} (REQUIRED)", memo),
            "Wait for confirmation".to_string(),
            format!(
                "NGN will be sent to your bank account ({})",
                verified_bank.account_number
            ),
        ],
        created_at: now.to_rfc3339(),
    };

    info!(
        transaction_id = %tx_id,
        quote_id = %quote.quote_id,
        "Offramp initiation successful"
    );

    (StatusCode::OK, Json(response)).into_response()
}

// ===== ERROR HANDLING =====

/// Handle offramp-specific errors
fn handle_offramp_error(error: AppError) -> Response {
    use crate::error::AppErrorKind::*;

    match error.kind {
        Validation(ve) => {
            use crate::error::ValidationError::*;
            match ve {
                InvalidWalletAddress { address, reason } => {
                    let detail = ErrorResponseDetail {
                        code: "WALLET_MISMATCH".to_string(),
                        message: reason,
                        details: None,
                        quote_id: None,
                        bank_code: None,
                        account_number: None,
                        provided_name: None,
                        actual_name: None,
                    };
                    (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse { error: detail }),
                    )
                        .into_response()
                }
                InvalidAmount { amount, reason } => {
                    match amount.parse::<u32>() {
                        Ok(_) => {
                            // Invalid bank code
                            let detail = ErrorResponseDetail {
                                code: "INVALID_BANK_CODE".to_string(),
                                message: reason,
                                details: Some("Bank code not supported".to_string()),
                                quote_id: None,
                                bank_code: Some(amount),
                                account_number: None,
                                provided_name: None,
                                actual_name: None,
                            };
                            (
                                StatusCode::BAD_REQUEST,
                                Json(ErrorResponse { error: detail }),
                            )
                                .into_response()
                        }
                        Err(_) => {
                            // Invalid account number or quote ID
                            let detail = ErrorResponseDetail {
                                code: if amount.len() == 10 {
                                    "INVALID_ACCOUNT_NUMBER".to_string()
                                } else {
                                    "INVALID_QUOTE".to_string()
                                },
                                message: reason,
                                details: None,
                                quote_id: None,
                                bank_code: None,
                                account_number: if amount.len() == 10 {
                                    Some(amount)
                                } else {
                                    None
                                },
                                provided_name: None,
                                actual_name: None,
                            };
                            (
                                StatusCode::BAD_REQUEST,
                                Json(ErrorResponse { error: detail }),
                            )
                                .into_response()
                        }
                    }
                }
                MissingField { field } => {
                    let detail = ErrorResponseDetail {
                        code: "MISSING_FIELD".to_string(),
                        message: format!("Required field missing: {}", field),
                        details: None,
                        quote_id: None,
                        bank_code: None,
                        account_number: None,
                        provided_name: None,
                        actual_name: None,
                    };
                    (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse { error: detail }),
                    )
                        .into_response()
                }
                _ => {
                    let detail = ErrorResponseDetail {
                        code: "VALIDATION_ERROR".to_string(),
                        message: "Validation failed".to_string(),
                        details: None,
                        quote_id: None,
                        bank_code: None,
                        account_number: None,
                        provided_name: None,
                        actual_name: None,
                    };
                    (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse { error: detail }),
                    )
                        .into_response()
                }
            }
        }
        Domain(de) => {
            use crate::error::DomainError::*;
            match de {
                RateExpired { quote_id } => {
                    let detail = ErrorResponseDetail {
                        code: "QUOTE_EXPIRED".to_string(),
                        message: "Quote has expired. Please generate a new quote.".to_string(),
                        details: Some("Quotes are only valid for 5 minutes.".to_string()),
                        quote_id: Some(quote_id),
                        bank_code: None,
                        account_number: None,
                        provided_name: None,
                        actual_name: None,
                    };
                    (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse { error: detail }),
                    )
                        .into_response()
                }
                _ => {
                    let detail = ErrorResponseDetail {
                        code: "INVALID_REQUEST".to_string(),
                        message: "Invalid request".to_string(),
                        details: None,
                        quote_id: None,
                        bank_code: None,
                        account_number: None,
                        provided_name: None,
                        actual_name: None,
                    };
                    (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse { error: detail }),
                    )
                        .into_response()
                }
            }
        }
        Infrastructure(ie) => {
            error!(error = ?ie, "Infrastructure error in offramp initiate");
            let detail = ErrorResponseDetail {
                code: "SERVICE_UNAVAILABLE".to_string(),
                message: "Unable to process request at this time".to_string(),
                details: Some("Please try again in a few moments".to_string()),
                quote_id: None,
                bank_code: None,
                account_number: None,
                provided_name: None,
                actual_name: None,
            };
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse { error: detail }),
            )
                .into_response()
        }
        External(ee) => {
            use crate::error::ExternalError::*;
            error!(error = ?ee, "External service error in offramp initiate");
            match ee {
                PaymentProvider {
                    provider,
                    message,
                    is_retryable,
                } => {
                    let (code, msg): (&str, String) = if message.contains("not found")
                        || message.contains("Account not found")
                    {
                        (
                            "INVALID_BANK_ACCOUNT",
                            "Bank account not found. Please verify account number and bank code."
                                .to_string(),
                        )
                    } else if message.contains("name mismatch") || message.contains("Name mismatch")
                    {
                        ("ACCOUNT_NAME_MISMATCH", message.clone())
                    } else {
                        (
                            "BANK_VERIFICATION_FAILED",
                            "Bank account verification failed".to_string(),
                        )
                    };

                    let status = if is_retryable {
                        StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        StatusCode::BAD_REQUEST
                    };

                    let detail = ErrorResponseDetail {
                        code: code.to_string(),
                        message: msg,
                        details: Some(format!("Provider: {}", provider)),
                        quote_id: None,
                        bank_code: None,
                        account_number: None,
                        provided_name: None,
                        actual_name: None,
                    };
                    (status, Json(ErrorResponse { error: detail })).into_response()
                }
                Timeout {
                    service,
                    timeout_secs,
                } => {
                    let detail = ErrorResponseDetail {
                        code: "VERIFICATION_TIMEOUT".to_string(),
                        message: format!(
                            "{} verification timed out after {} seconds",
                            service, timeout_secs
                        ),
                        details: Some("Please try again".to_string()),
                        quote_id: None,
                        bank_code: None,
                        account_number: None,
                        provided_name: None,
                        actual_name: None,
                    };
                    (
                        StatusCode::GATEWAY_TIMEOUT,
                        Json(ErrorResponse { error: detail }),
                    )
                        .into_response()
                }
                _ => {
                    let detail = ErrorResponseDetail {
                        code: "VERIFICATION_SERVICE_UNAVAILABLE".to_string(),
                        message: "Unable to verify bank account at this time".to_string(),
                        details: Some(
                            "Bank verification service is temporarily unavailable".to_string(),
                        ),
                        quote_id: None,
                        bank_code: None,
                        account_number: None,
                        provided_name: None,
                        actual_name: None,
                    };
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(ErrorResponse { error: detail }),
                    )
                        .into_response()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_bank_code_valid() {
        assert!(validate_bank_code("044").is_ok()); // Access Bank
        assert!(validate_bank_code("058").is_ok()); // GTBank
        assert!(validate_bank_code("033").is_ok()); // UBA
    }

    #[test]
    fn test_validate_bank_code_invalid() {
        assert!(validate_bank_code("999").is_err());
        assert!(validate_bank_code("abc").is_err());
    }

    #[test]
    fn test_validate_account_number_valid() {
        assert!(validate_account_number("0123456789").is_ok());
        assert!(validate_account_number("1234567890").is_ok());
    }

    #[test]
    fn test_validate_account_number_invalid() {
        assert!(validate_account_number("123456789").is_err()); // 9 digits
        assert!(validate_account_number("12345678901").is_err()); // 11 digits
        assert!(validate_account_number("012345678a").is_err()); // non-digit
    }

    #[test]
    fn test_validate_account_name() {
        assert!(validate_account_name("John Doe").is_ok());
        assert!(validate_account_name("JANE SMITH").is_ok());
        assert!(validate_account_name("").is_err());
    }

    #[test]
    fn test_generate_withdrawal_memo() {
        let uuid = Uuid::new_v4();
        let memo = generate_withdrawal_memo(&uuid);

        // Check format: WD-{8_chars}
        assert!(memo.starts_with("WD-"), "Memo should start with WD-");
        assert_eq!(
            memo.len(),
            11,
            "Memo should be 11 characters (WD- + 8 UUID chars)"
        );

        // Check it's ASCII and within Stellar memo limit
        assert!(memo.is_ascii(), "Memo should be ASCII");
        assert!(
            memo.len() <= 28,
            "Memo should be under Stellar 28-byte limit"
        );

        // Check format is uppercase
        assert_eq!(memo, memo.to_uppercase(), "Memo should be uppercase");

        // Check all characters after WD- are hex digits
        let hex_part = &memo[3..];
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "Characters after WD- should be hex digits"
        );
    }

    #[test]
    fn test_memo_uniqueness() {
        let uuid1 = Uuid::new_v4();
        let uuid2 = Uuid::new_v4();

        let memo1 = generate_withdrawal_memo(&uuid1);
        let memo2 = generate_withdrawal_memo(&uuid2);

        // Memos should be different (with extremely high probability)
        assert_ne!(
            memo1, memo2,
            "Different UUIDs should generate different memos"
        );
    }

    #[test]
    fn test_memo_reproducibility() {
        let uuid = uuid::Uuid::parse_str("9f8e7d6c-5b4a-1234-a5b6-c7d8e9f0a1b2").unwrap();
        let memo = generate_withdrawal_memo(&uuid);

        // Should produce consistent output
        let memo2 = generate_withdrawal_memo(&uuid);
        assert_eq!(memo, memo2, "Same UUID should produce same memo");

        // Check expected format
        assert_eq!(&memo[0..3], "WD-", "Should start with WD-");
        assert_eq!(memo.len(), 11, "Should be 11 characters total");
    }
}
