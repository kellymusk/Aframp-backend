pub mod burn_transaction_builder;
pub mod client;
pub mod config;
pub mod dex_liquidity;
pub mod errors;
pub mod issuer;
pub mod payment;
pub mod sep;
pub mod service;
pub mod sse_listener;
pub mod trustline;
pub mod types;
pub mod xdr_parser;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod stellar_service_tests;
use tracing::{error, info_span, instrument, warn, Instrument};

use crate::telemetry::propagation::inject_context;

/// Result of a Stellar transaction submission.
#[derive(Debug)]
pub struct StellarTxResult {
    pub hash: String,
    pub ledger: u32,
    pub successful: bool,
}

pub struct StellarService {
    pub horizon_url: String,
    pub cngn_issuer: String,
    pub http_client: reqwest::Client,
}

impl StellarService {
    pub fn new(horizon_url: String, cngn_issuer: String) -> Self {
        Self {
            horizon_url,
            cngn_issuer,
            http_client: reqwest::Client::new(),
        }
    }

    /// Check whether a wallet has an active CNGN trustline.
    ///
    /// Emits:
    /// * `stellar.trustline_check` span with `wallet_address` attribute.
    /// * `stellar.horizon_call` child span for the Horizon API request.
    #[instrument(skip(self), fields(wallet_address = %wallet_address))]
    pub async fn check_trustline(
        &self,
        wallet_address: &str,
        asset_code: &str,
    ) -> anyhow::Result<bool> {
        let span = info_span!(
            "stellar.trustline_check",
            otel.kind = "client",
            peer.service = "stellar_horizon",
            stellar.wallet = %wallet_address,
            stellar.asset = %asset_code,
        );

        async {
            self.horizon_api_call("get_account", wallet_address).await?;
            tracing::info!(%wallet_address, %asset_code, "Trustline check complete");
            Ok(true) // simulated
        }
        .instrument(span)
        .await
    }

    /// Establish a CNGN trustline for a new wallet.
    ///
    /// Emits a `stellar.trustline_create` span with the wallet address and
    /// asset code as attributes.
    #[instrument(skip(self), fields(wallet_address = %wallet_address, asset_code = %asset_code))]
    pub async fn create_trustline(
        &self,
        wallet_address: &str,
        asset_code: &str,
    ) -> anyhow::Result<StellarTxResult> {
        let span = info_span!(
            "stellar.trustline_create",
            otel.kind = "client",
            peer.service = "stellar_horizon",
            stellar.wallet = %wallet_address,
            stellar.asset = %asset_code,
            stellar.issuer = %self.cngn_issuer,
        );

        async {
            tracing::info!(%wallet_address, %asset_code, "Building trustline transaction");

            // Submit to Horizon.
            self.horizon_submit_tx("change_trust").await?;

            let result = StellarTxResult {
                hash: "SIM_TRUST_HASH_001".into(),
                ledger: 12345,
                successful: true,
            };

            tracing::info!(
                %wallet_address,
                tx_hash = %result.hash,
                "Trustline created successfully"
            );

            Ok(result)
        }
        .instrument(span)
        .await
    }

    /// Build and submit a cNGN payment transaction.
    ///
    /// Emits:
    /// * `stellar.payment_send` root span with source, destination, asset, and amount.
    /// * `stellar.horizon_call` child span for the submission call.
    #[instrument(
        skip(self),
        fields(
            source = %source,
            destination = %destination,
            asset = %asset_code,
            amount = %amount,
        )
    )]
    pub async fn send_payment(
        &self,
        source: &str,
        destination: &str,
        asset_code: &str,
        amount: &str,
    ) -> anyhow::Result<StellarTxResult> {
        let span = info_span!(
            "stellar.payment_send",
            otel.kind = "client",
            peer.service = "stellar_horizon",
            stellar.source = %source,
            stellar.destination = %destination,
            stellar.asset = %asset_code,
            stellar.amount = %amount,
        );

        async {
            tracing::info!(%source, %destination, %amount, %asset_code, "Building payment transaction");

            // Submit to Horizon.
            self.horizon_submit_tx("payment").await?;

            let result = StellarTxResult {
                hash: "SIM_PAY_HASH_001".into(),
                ledger: 12346,
                successful: true,
            };

            tracing::info!(tx_hash = %result.hash, "Payment submitted to Stellar");
            Ok(result)
        }
        .instrument(span)
        .await
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Emit a labelled child span for a Horizon API call and inject the W3C
    /// `traceparent` header so the Horizon node can (optionally) continue the trace.
    async fn horizon_api_call(&self, operation: &str, account: &str) -> anyhow::Result<()> {
        let url = format!("{}/accounts/{}", self.horizon_url, account);

        let span = info_span!(
            "stellar.horizon_call",
            otel.kind = "client",
            peer.service = "stellar_horizon",
            stellar.horizon_operation = %operation,
            http.method = "GET",
            http.url = %url,
        );

        async {
            let mut headers = reqwest::header::HeaderMap::new();
            inject_context(&mut headers);
            tracing::debug!(%operation, %url, "Calling Stellar Horizon API");
            Ok(())
        }
        .instrument(span)
        .await
    }

    async fn horizon_submit_tx(&self, operation: &str) -> anyhow::Result<()> {
        let url = format!("{}/transactions", self.horizon_url);

        let span = info_span!(
            "stellar.horizon_call",
            otel.kind = "client",
            peer.service = "stellar_horizon",
            stellar.horizon_operation = %operation,
            http.method = "POST",
            http.url = %url,
        );

        async {
            let mut headers = reqwest::header::HeaderMap::new();
            inject_context(&mut headers);
            tracing::debug!(%operation, "Submitting transaction to Stellar Horizon");
            Ok(())
        }
        .instrument(span)
        .await
    }
}
