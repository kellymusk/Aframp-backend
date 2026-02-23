use crate::payments::error::{PaymentError, PaymentResult};
use crate::payments::provider::PaymentProvider;
use crate::payments::providers::{FlutterwaveProvider, MpesaProvider, PaystackProvider};
use crate::payments::types::ProviderName;
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct PaymentFactoryConfig {
    pub default_provider: ProviderName,
    pub enabled_providers: Vec<ProviderName>,
    pub provider_fee_bps: HashMap<ProviderName, u32>,
}

impl PaymentFactoryConfig {
    pub fn from_env() -> PaymentResult<Self> {
        let default_provider =
            std::env::var("DEFAULT_PAYMENT_PROVIDER").unwrap_or_else(|_| "paystack".to_string());
        let default_provider = ProviderName::from_str(&default_provider)?;

        let enabled_raw = std::env::var("ENABLED_PAYMENT_PROVIDERS")
            .unwrap_or_else(|_| "paystack,flutterwave,mpesa".to_string());
        let mut enabled_providers = Vec::new();
        for part in enabled_raw.split(',') {
            let value = part.trim();
            if value.is_empty() {
                continue;
            }
            enabled_providers.push(ProviderName::from_str(value)?);
        }

        if !enabled_providers.contains(&default_provider) {
            return Err(PaymentError::ValidationError {
                message: "default provider must be enabled".to_string(),
                field: Some("DEFAULT_PAYMENT_PROVIDER".to_string()),
            });
        }

        let mut provider_fee_bps = HashMap::new();
        provider_fee_bps.insert(
            ProviderName::Paystack,
            std::env::var("PAYSTACK_FEE_BPS")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(150),
        );
        provider_fee_bps.insert(
            ProviderName::Flutterwave,
            std::env::var("FLUTTERWAVE_FEE_BPS")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(155),
        );
        provider_fee_bps.insert(
            ProviderName::Mpesa,
            std::env::var("MPESA_FEE_BPS")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(170),
        );

        Ok(Self {
            default_provider,
            enabled_providers,
            provider_fee_bps,
        })
    }
}

pub struct PaymentProviderFactory {
    config: PaymentFactoryConfig,
}

impl PaymentProviderFactory {
    pub fn from_env() -> PaymentResult<Self> {
        let config = PaymentFactoryConfig::from_env()?;
        Ok(Self { config })
    }

    pub fn with_config(config: PaymentFactoryConfig) -> Self {
        Self { config }
    }

    pub fn get_provider(&self, provider: ProviderName) -> PaymentResult<Box<dyn PaymentProvider>> {
        if !self.config.enabled_providers.contains(&provider) {
            return Err(PaymentError::ValidationError {
                message: format!("provider {} is disabled", provider),
                field: Some("provider".to_string()),
            });
        }

        match provider {
            ProviderName::Paystack => Ok(Box::new(PaystackProvider::from_env()?)),
            ProviderName::Flutterwave => Ok(Box::new(FlutterwaveProvider::from_env()?)),
            ProviderName::Mpesa => Ok(Box::new(MpesaProvider::from_env()?)),
        }
    }

    pub fn get_default_provider(&self) -> PaymentResult<Box<dyn PaymentProvider>> {
        self.get_provider(self.config.default_provider.clone())
    }

    pub fn get_default_for_country(
        &self,
        country_code: &str,
    ) -> PaymentResult<Box<dyn PaymentProvider>> {
        let normalized = country_code.trim().to_uppercase();
        let provider = match normalized.as_str() {
            "NG" => ProviderName::Paystack,
            "KE" | "TZ" | "UG" => ProviderName::Mpesa,
            _ => self.config.default_provider.clone(),
        };
        self.get_provider(provider)
    }

    pub fn get_cheapest_provider_for_amount(
        &self,
        _amount_minor_units: i64,
    ) -> PaymentResult<Box<dyn PaymentProvider>> {
        let cheapest = self
            .config
            .enabled_providers
            .iter()
            .min_by_key(|p| {
                self.config
                    .provider_fee_bps
                    .get(*p)
                    .copied()
                    .unwrap_or(u32::MAX)
            })
            .cloned()
            .unwrap_or(self.config.default_provider.clone());
        self.get_provider(cheapest)
    }

    pub fn list_available_providers(&self) -> Vec<ProviderName> {
        self.config.enabled_providers.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_name_parsing_works() {
        assert!(matches!(
            ProviderName::from_str("paystack"),
            Ok(ProviderName::Paystack)
        ));
        assert!(ProviderName::from_str("unknown").is_err());
    }

    #[test]
    fn list_available_providers_returns_enabled() {
        let factory = PaymentProviderFactory::with_config(PaymentFactoryConfig {
            default_provider: ProviderName::Paystack,
            enabled_providers: vec![ProviderName::Paystack, ProviderName::Flutterwave],
            provider_fee_bps: HashMap::new(),
        });
        let providers = factory.list_available_providers();
        assert_eq!(providers.len(), 2);
    }
}
