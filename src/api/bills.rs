//! Bill payment providers API
//!
//! Provides a public endpoint to list available bill payment providers in Nigeria.
//! Users can discover what services they can pay for using cNGN.

use axum::{extract::Query, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::info;

/// Supported provider categories
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCategory {
    Electricity,
    Airtime,
    Data,
    CableTv,
    Internet,
}

impl ProviderCategory {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "electricity" => Some(Self::Electricity),
            "airtime" => Some(Self::Airtime),
            "data" => Some(Self::Data),
            "cable_tv" | "cable" => Some(Self::CableTv),
            "internet" => Some(Self::Internet),
            _ => None,
        }
    }

    pub fn to_string(&self) -> String {
        match self {
            Self::Electricity => "electricity".to_string(),
            Self::Airtime => "airtime".to_string(),
            Self::Data => "data".to_string(),
            Self::CableTv => "cable_tv".to_string(),
            Self::Internet => "internet".to_string(),
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Electricity => "Electricity",
            Self::Airtime => "Airtime",
            Self::Data => "Data Bundles",
            Self::CableTv => "Cable TV",
            Self::Internet => "Internet",
        }
    }
}

/// Provider status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderStatus {
    Active,
    Inactive,
    Maintenance,
}

impl ProviderStatus {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "active" => Some(Self::Active),
            "inactive" => Some(Self::Inactive),
            "maintenance" => Some(Self::Maintenance),
            _ => None,
        }
    }
}

/// Field type for required fields
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Text,
    Number,
    Tel,
    Select,
    Email,
}

/// Field validation rules
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FieldValidation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length: Option<usize>,
}

/// Option for select fields
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Required field for a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredField {
    pub field_name: String,
    pub field_label: String,
    pub field_type: FieldType,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation: Option<FieldValidation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<FieldOption>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

/// Amount limits for a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmountLimits {
    pub min_amount: String,
    pub max_amount: String,
    pub currency: String,
    pub fixed_amounts: bool,
}

/// Processing information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingInfo {
    pub estimated_time: String,
    pub fees: ProcessingFees,
}

/// Processing fees
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingFees {
    #[serde(rename = "service_fee")]
    pub service_fee: String,
    #[serde(rename = "convenience_fee_percentage")]
    pub convenience_fee_percentage: f64,
}

/// Bill provider data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BillProvider {
    #[serde(rename = "provider_id")]
    pub provider_id: String,
    #[serde(rename = "provider_code")]
    pub provider_code: String,
    pub name: String,
    #[serde(rename = "short_name")]
    pub short_name: String,
    pub category: ProviderCategory,
    pub description: String,
    #[serde(rename = "logo_url")]
    pub logo_url: String,
    pub status: ProviderStatus,
    #[serde(rename = "supported_currencies")]
    pub supported_currencies: Vec<String>,
    #[serde(rename = "required_fields")]
    pub required_fields: Vec<RequiredField>,
    #[serde(rename = "amount_limits")]
    pub amount_limits: AmountLimits,
    pub processing: ProcessingInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help_url: Option<String>,
}

/// Category summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategorySummary {
    #[serde(rename = "category_id")]
    pub category_id: String,
    pub name: String,
    pub count: usize,
}

/// Query parameters for the providers endpoint
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProviderQuery {
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub group_by: Option<String>,
}

/// Error response for invalid category
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetails {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_categories: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_countries: Option<Vec<String>>,
}

/// Response for providers endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersResponse {
    pub country: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub providers: Vec<BillProvider>,
    #[serde(rename = "total_providers")]
    pub total_providers: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<CategorySummary>>,
}

/// Get all bill payment providers
pub async fn get_providers(Query(query): Query<ProviderQuery>) -> impl IntoResponse {
    info!(
        country = ?query.country,
        category = ?query.category,
        status = ?query.status,
        group_by = ?query.group_by,
        "Fetching bill providers"
    );

    // Validate country (only NG supported for now)
    let country = query.country.unwrap_or_else(|| "NG".to_string());
    if country != "NG" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: ErrorDetails {
                    code: "UNSUPPORTED_COUNTRY".to_string(),
                    message: format!("Bill payments not available in country: {}", country),
                    supported_countries: Some(vec!["NG".to_string()]),
                    supported_categories: None,
                },
            }),
        )
            .into_response();
    }

    // Validate category if provided
    if let Some(ref cat) = query.category {
        if ProviderCategory::from_str(cat).is_none() {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: ErrorDetails {
                        code: "INVALID_CATEGORY".to_string(),
                        message: format!("Invalid category: {}", cat),
                        supported_categories: Some(vec![
                            "electricity".to_string(),
                            "airtime".to_string(),
                            "data".to_string(),
                            "cable_tv".to_string(),
                        ]),
                        supported_countries: None,
                    },
                }),
            )
                .into_response();
        }
    }

    // Get all providers
    let mut providers = get_all_providers();

    // Filter by category
    if let Some(ref cat) = query.category {
        if let Some(category) = ProviderCategory::from_str(cat) {
            providers.retain(|p| p.category == category);
        }
    }

    // Filter by status (default: active)
    let status_filter = query.status.as_deref().unwrap_or("active");
    if status_filter != "all" {
        if let Some(status) = ProviderStatus::from_str(status_filter) {
            providers.retain(|p| p.status == status);
        }
    }

    // Build category summary
    let categories = build_category_summary(&get_all_providers());

    let total_providers = providers.len();

    // Check if we should group by category
    if query.group_by.as_deref() == Some("category") {
        return (
            StatusCode::OK,
            Json(ProvidersResponse {
                country,
                category: query.category,
                providers: vec![], // Empty when grouped
                total_providers,
                categories: Some(categories),
            }),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(ProvidersResponse {
            country,
            category: query.category,
            providers,
            total_providers,
            categories: Some(categories),
        }),
    )
        .into_response()
}

/// Group providers by category
fn group_providers_by_category(providers: Vec<BillProvider>) -> HashMap<String, Vec<BillProvider>> {
    let mut grouped: HashMap<String, Vec<BillProvider>> = HashMap::new();
    for provider in providers {
        let category = provider.category.to_string();
        grouped
            .entry(category)
            .or_insert_with(Vec::new)
            .push(provider);
    }
    grouped
}

/// Build category summary
fn build_category_summary(providers: &[BillProvider]) -> Vec<CategorySummary> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for provider in providers {
        let cat = provider.category.to_string();
        *counts.entry(cat).or_insert(0) += 1;
    }

    let mut categories = vec![
        ("electricity", "Electricity"),
        ("airtime", "Airtime"),
        ("data", "Data Bundles"),
        ("cable_tv", "Cable TV"),
    ];

    categories
        .iter()
        .map(|(id, name)| CategorySummary {
            category_id: id.to_string(),
            name: name.to_string(),
            count: *counts.get(*id).unwrap_or(&0),
        })
        .collect()
}

/// Get all bill payment providers (hardcoded for now - could be loaded from config)
pub fn get_all_providers() -> Vec<BillProvider> {
    vec![
        // ==================== ELECTRICITY PROVIDERS ====================
        BillProvider {
            provider_id: "ekedc".to_string(),
            provider_code: "ekedc-electric".to_string(),
            name: "Eko Electricity (EKEDC)".to_string(),
            short_name: "EKEDC".to_string(),
            category: ProviderCategory::Electricity,
            description: "Pay your Eko Electricity bills instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/ekedc.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string(), "NGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "meter_number".to_string(),
                    field_label: "Meter Number".to_string(),
                    field_type: FieldType::Text,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^[0-9]{10,13}$".to_string()),
                        min_length: Some(10),
                        max_length: Some(13),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter 10-13 digit meter number".to_string()),
                },
                RequiredField {
                    field_name: "meter_type".to_string(),
                    field_label: "Meter Type".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "prepaid".to_string(),
                            label: "Prepaid".to_string(),
                            amount: None,
                            description: None,
                        },
                        FieldOption {
                            value: "postpaid".to_string(),
                            label: "Postpaid".to_string(),
                            amount: None,
                            description: None,
                        },
                    ]),
                    placeholder: None,
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(500),
                        max: Some(100000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦500 - ₦100,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "100000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.5,
                },
            },
            help_url: Some("https://aframp.com/help/electricity-bills".to_string()),
        },
        BillProvider {
            provider_id: "ikedc".to_string(),
            provider_code: "ikedc-electric".to_string(),
            name: "Ikeja Electric (IKEDC)".to_string(),
            short_name: "IKEDC".to_string(),
            category: ProviderCategory::Electricity,
            description: "Pay your Ikeja Electric bills instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/ikedc.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string(), "NGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "meter_number".to_string(),
                    field_label: "Meter Number".to_string(),
                    field_type: FieldType::Text,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^[0-9]{10,13}$".to_string()),
                        min_length: Some(10),
                        max_length: Some(13),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter 10-13 digit meter number".to_string()),
                },
                RequiredField {
                    field_name: "meter_type".to_string(),
                    field_label: "Meter Type".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "prepaid".to_string(),
                            label: "Prepaid".to_string(),
                            amount: None,
                            description: None,
                        },
                        FieldOption {
                            value: "postpaid".to_string(),
                            label: "Postpaid".to_string(),
                            amount: None,
                            description: None,
                        },
                    ]),
                    placeholder: None,
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(500),
                        max: Some(100000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦500 - ₦100,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "100000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.5,
                },
            },
            help_url: Some("https://aframp.com/help/electricity-bills".to_string()),
        },
        BillProvider {
            provider_id: "aedc".to_string(),
            provider_code: "aedc-electric".to_string(),
            name: "Abuja Electricity (AEDC)".to_string(),
            short_name: "AEDC".to_string(),
            category: ProviderCategory::Electricity,
            description: "Pay your Abuja Electricity bills instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/aedc.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string(), "NGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "meter_number".to_string(),
                    field_label: "Meter Number".to_string(),
                    field_type: FieldType::Text,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^[0-9]{10,13}$".to_string()),
                        min_length: Some(10),
                        max_length: Some(13),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter 10-13 digit meter number".to_string()),
                },
                RequiredField {
                    field_name: "meter_type".to_string(),
                    field_label: "Meter Type".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "prepaid".to_string(),
                            label: "Prepaid".to_string(),
                            amount: None,
                            description: None,
                        },
                        FieldOption {
                            value: "postpaid".to_string(),
                            label: "Postpaid".to_string(),
                            amount: None,
                            description: None,
                        },
                    ]),
                    placeholder: None,
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(500),
                        max: Some(100000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦500 - ₦100,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "100000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.5,
                },
            },
            help_url: Some("https://aframp.com/help/electricity-bills".to_string()),
        },
        BillProvider {
            provider_id: "phed".to_string(),
            provider_code: "phed-electric".to_string(),
            name: "Port Harcourt Electric (PHED)".to_string(),
            short_name: "PHED".to_string(),
            category: ProviderCategory::Electricity,
            description: "Pay your Port Harcourt Electricity bills instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/phed.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string(), "NGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "meter_number".to_string(),
                    field_label: "Meter Number".to_string(),
                    field_type: FieldType::Text,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^[0-9]{10,13}$".to_string()),
                        min_length: Some(10),
                        max_length: Some(13),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter 10-13 digit meter number".to_string()),
                },
                RequiredField {
                    field_name: "meter_type".to_string(),
                    field_label: "Meter Type".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "prepaid".to_string(),
                            label: "Prepaid".to_string(),
                            amount: None,
                            description: None,
                        },
                        FieldOption {
                            value: "postpaid".to_string(),
                            label: "Postpaid".to_string(),
                            amount: None,
                            description: None,
                        },
                    ]),
                    placeholder: None,
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(500),
                        max: Some(100000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦500 - ₦100,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "100000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.5,
                },
            },
            help_url: Some("https://aframp.com/help/electricity-bills".to_string()),
        },
        BillProvider {
            provider_id: "ibedc".to_string(),
            provider_code: "ibedc-electric".to_string(),
            name: "Ibadan Electricity (IBEDC)".to_string(),
            short_name: "IBEDC".to_string(),
            category: ProviderCategory::Electricity,
            description: "Pay your Ibadan Electricity bills instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/ibedc.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string(), "NGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "meter_number".to_string(),
                    field_label: "Meter Number".to_string(),
                    field_type: FieldType::Text,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^[0-9]{10,13}$".to_string()),
                        min_length: Some(10),
                        max_length: Some(13),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter 10-13 digit meter number".to_string()),
                },
                RequiredField {
                    field_name: "meter_type".to_string(),
                    field_label: "Meter Type".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "prepaid".to_string(),
                            label: "Prepaid".to_string(),
                            amount: None,
                            description: None,
                        },
                        FieldOption {
                            value: "postpaid".to_string(),
                            label: "Postpaid".to_string(),
                            amount: None,
                            description: None,
                        },
                    ]),
                    placeholder: None,
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(500),
                        max: Some(100000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦500 - ₦100,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "100000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.5,
                },
            },
            help_url: Some("https://aframp.com/help/electricity-bills".to_string()),
        },
        BillProvider {
            provider_id: "jed".to_string(),
            provider_code: "jed-electric".to_string(),
            name: "Jos Electricity (JED)".to_string(),
            short_name: "JED".to_string(),
            category: ProviderCategory::Electricity,
            description: "Pay your Jos Electricity bills instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/jed.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string(), "NGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "meter_number".to_string(),
                    field_label: "Meter Number".to_string(),
                    field_type: FieldType::Text,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^[0-9]{10,13}$".to_string()),
                        min_length: Some(10),
                        max_length: Some(13),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter 10-13 digit meter number".to_string()),
                },
                RequiredField {
                    field_name: "meter_type".to_string(),
                    field_label: "Meter Type".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "prepaid".to_string(),
                            label: "Prepaid".to_string(),
                            amount: None,
                            description: None,
                        },
                        FieldOption {
                            value: "postpaid".to_string(),
                            label: "Postpaid".to_string(),
                            amount: None,
                            description: None,
                        },
                    ]),
                    placeholder: None,
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(500),
                        max: Some(100000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦500 - ₦100,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "100000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.5,
                },
            },
            help_url: Some("https://aframp.com/help/electricity-bills".to_string()),
        },
        BillProvider {
            provider_id: "kaedco".to_string(),
            provider_code: "kaedco-electric".to_string(),
            name: "Kaduna Electric (KAEDCO)".to_string(),
            short_name: "KAEDCO".to_string(),
            category: ProviderCategory::Electricity,
            description: "Pay your Kaduna Electric bills instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/kaedco.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string(), "NGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "meter_number".to_string(),
                    field_label: "Meter Number".to_string(),
                    field_type: FieldType::Text,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^[0-9]{10,13}$".to_string()),
                        min_length: Some(10),
                        max_length: Some(13),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter 10-13 digit meter number".to_string()),
                },
                RequiredField {
                    field_name: "meter_type".to_string(),
                    field_label: "Meter Type".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "prepaid".to_string(),
                            label: "Prepaid".to_string(),
                            amount: None,
                            description: None,
                        },
                        FieldOption {
                            value: "postpaid".to_string(),
                            label: "Postpaid".to_string(),
                            amount: None,
                            description: None,
                        },
                    ]),
                    placeholder: None,
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(500),
                        max: Some(100000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦500 - ₦100,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "100000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.5,
                },
            },
            help_url: Some("https://aframp.com/help/electricity-bills".to_string()),
        },
        BillProvider {
            provider_id: "kedco".to_string(),
            provider_code: "kedco-electric".to_string(),
            name: "Kano Electricity (KEDCO)".to_string(),
            short_name: "KEDCO".to_string(),
            category: ProviderCategory::Electricity,
            description: "Pay your Kano Electricity bills instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/kedco.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string(), "NGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "meter_number".to_string(),
                    field_label: "Meter Number".to_string(),
                    field_type: FieldType::Text,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^[0-9]{10,13}$".to_string()),
                        min_length: Some(10),
                        max_length: Some(13),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter 10-13 digit meter number".to_string()),
                },
                RequiredField {
                    field_name: "meter_type".to_string(),
                    field_label: "Meter Type".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "prepaid".to_string(),
                            label: "Prepaid".to_string(),
                            amount: None,
                            description: None,
                        },
                        FieldOption {
                            value: "postpaid".to_string(),
                            label: "Postpaid".to_string(),
                            amount: None,
                            description: None,
                        },
                    ]),
                    placeholder: None,
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(500),
                        max: Some(100000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦500 - ₦100,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "100000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.5,
                },
            },
            help_url: Some("https://aframp.com/help/electricity-bills".to_string()),
        },
        // ==================== AIRTIME PROVIDERS ====================
        BillProvider {
            provider_id: "mtn-airtime".to_string(),
            provider_code: "mtn-ng".to_string(),
            name: "MTN Airtime".to_string(),
            short_name: "MTN".to_string(),
            category: ProviderCategory::Airtime,
            description: "Buy MTN airtime instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/mtn.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "phone_number".to_string(),
                    field_label: "Phone Number".to_string(),
                    field_type: FieldType::Tel,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^(080|090|070|081)[0-9]{8}$".to_string()),
                        format: Some("080XXXXXXXX".to_string()),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("08012345678".to_string()),
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(50),
                        max: Some(10000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦50 - ₦10,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "50.00".to_string(),
                max_amount: "10000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.0,
                },
            },
            help_url: Some("https://aframp.com/help/airtime".to_string()),
        },
        BillProvider {
            provider_id: "airtel-airtime".to_string(),
            provider_code: "airtel-ng".to_string(),
            name: "Airtel Airtime".to_string(),
            short_name: "Airtel".to_string(),
            category: ProviderCategory::Airtime,
            description: "Buy Airtel airtime instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/airtel.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "phone_number".to_string(),
                    field_label: "Phone Number".to_string(),
                    field_type: FieldType::Tel,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^(080|090|070|081)[0-9]{8}$".to_string()),
                        format: Some("080XXXXXXXX".to_string()),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("08012345678".to_string()),
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(50),
                        max: Some(10000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦50 - ₦10,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "50.00".to_string(),
                max_amount: "10000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.0,
                },
            },
            help_url: Some("https://aframp.com/help/airtime".to_string()),
        },
        BillProvider {
            provider_id: "glo-airtime".to_string(),
            provider_code: "glo-ng".to_string(),
            name: "Glo Airtime".to_string(),
            short_name: "Glo".to_string(),
            category: ProviderCategory::Airtime,
            description: "Buy Glo airtime instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/glo.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "phone_number".to_string(),
                    field_label: "Phone Number".to_string(),
                    field_type: FieldType::Tel,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^(080|090|070|081)[0-9]{8}$".to_string()),
                        format: Some("080XXXXXXXX".to_string()),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("08012345678".to_string()),
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(50),
                        max: Some(10000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦50 - ₦10,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "50.00".to_string(),
                max_amount: "10000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.0,
                },
            },
            help_url: Some("https://aframp.com/help/airtime".to_string()),
        },
        BillProvider {
            provider_id: "9mobile-airtime".to_string(),
            provider_code: "9mobile-ng".to_string(),
            name: "9mobile Airtime".to_string(),
            short_name: "9mobile".to_string(),
            category: ProviderCategory::Airtime,
            description: "Buy 9mobile airtime instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/9mobile.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "phone_number".to_string(),
                    field_label: "Phone Number".to_string(),
                    field_type: FieldType::Tel,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^(080|090|070|081)[0-9]{8}$".to_string()),
                        format: Some("080XXXXXXXX".to_string()),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("08012345678".to_string()),
                },
                RequiredField {
                    field_name: "amount".to_string(),
                    field_label: "Amount".to_string(),
                    field_type: FieldType::Number,
                    required: true,
                    validation: Some(FieldValidation {
                        min: Some(50),
                        max: Some(10000),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("Enter amount (₦50 - ₦10,000)".to_string()),
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "50.00".to_string(),
                max_amount: "10000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: false,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.0,
                },
            },
            help_url: Some("https://aframp.com/help/airtime".to_string()),
        },
        // ==================== DATA PROVIDERS ====================
        BillProvider {
            provider_id: "mtn-data".to_string(),
            provider_code: "mtn-data-ng".to_string(),
            name: "MTN Data".to_string(),
            short_name: "MTN Data".to_string(),
            category: ProviderCategory::Data,
            description: "Buy MTN data bundles instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/mtn.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "phone_number".to_string(),
                    field_label: "Phone Number".to_string(),
                    field_type: FieldType::Tel,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^(080|090|070|081)[0-9]{8}$".to_string()),
                        format: Some("080XXXXXXXX".to_string()),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("08012345678".to_string()),
                },
                RequiredField {
                    field_name: "plan_code".to_string(),
                    field_label: "Data Plan".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "mtn-500mb".to_string(),
                            label: "500MB".to_string(),
                            amount: Some("500.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "mtn-1gb".to_string(),
                            label: "1GB".to_string(),
                            amount: Some("1000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "mtn-2gb".to_string(),
                            label: "2GB".to_string(),
                            amount: Some("2000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "mtn-5gb".to_string(),
                            label: "5GB".to_string(),
                            amount: Some("5000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "mtn-10gb".to_string(),
                            label: "10GB".to_string(),
                            amount: Some("10000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                    ]),
                    placeholder: None,
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "10000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: true,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.0,
                },
            },
            help_url: Some("https://aframp.com/help/data".to_string()),
        },
        BillProvider {
            provider_id: "airtel-data".to_string(),
            provider_code: "airtel-data-ng".to_string(),
            name: "Airtel Data".to_string(),
            short_name: "Airtel Data".to_string(),
            category: ProviderCategory::Data,
            description: "Buy Airtel data bundles instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/airtel.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "phone_number".to_string(),
                    field_label: "Phone Number".to_string(),
                    field_type: FieldType::Tel,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^(080|090|070|081)[0-9]{8}$".to_string()),
                        format: Some("080XXXXXXXX".to_string()),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("08012345678".to_string()),
                },
                RequiredField {
                    field_name: "plan_code".to_string(),
                    field_label: "Data Plan".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "airtel-500mb".to_string(),
                            label: "500MB".to_string(),
                            amount: Some("500.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "airtel-1gb".to_string(),
                            label: "1GB".to_string(),
                            amount: Some("1000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "airtel-2gb".to_string(),
                            label: "2GB".to_string(),
                            amount: Some("2000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "airtel-5gb".to_string(),
                            label: "5GB".to_string(),
                            amount: Some("5000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "airtel-10gb".to_string(),
                            label: "10GB".to_string(),
                            amount: Some("10000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                    ]),
                    placeholder: None,
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "10000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: true,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.0,
                },
            },
            help_url: Some("https://aframp.com/help/data".to_string()),
        },
        BillProvider {
            provider_id: "glo-data".to_string(),
            provider_code: "glo-data-ng".to_string(),
            name: "Glo Data".to_string(),
            short_name: "Glo Data".to_string(),
            category: ProviderCategory::Data,
            description: "Buy Glo data bundles instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/glo.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "phone_number".to_string(),
                    field_label: "Phone Number".to_string(),
                    field_type: FieldType::Tel,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^(080|090|070|081)[0-9]{8}$".to_string()),
                        format: Some("080XXXXXXXX".to_string()),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("08012345678".to_string()),
                },
                RequiredField {
                    field_name: "plan_code".to_string(),
                    field_label: "Data Plan".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "glo-500mb".to_string(),
                            label: "500MB".to_string(),
                            amount: Some("500.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "glo-1gb".to_string(),
                            label: "1GB".to_string(),
                            amount: Some("1000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "glo-2gb".to_string(),
                            label: "2GB".to_string(),
                            amount: Some("2000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "glo-5gb".to_string(),
                            label: "5GB".to_string(),
                            amount: Some("5000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "glo-10gb".to_string(),
                            label: "10GB".to_string(),
                            amount: Some("10000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                    ]),
                    placeholder: None,
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "10000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: true,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.0,
                },
            },
            help_url: Some("https://aframp.com/help/data".to_string()),
        },
        BillProvider {
            provider_id: "9mobile-data".to_string(),
            provider_code: "9mobile-data-ng".to_string(),
            name: "9mobile Data".to_string(),
            short_name: "9mobile Data".to_string(),
            category: ProviderCategory::Data,
            description: "Buy 9mobile data bundles instantly".to_string(),
            logo_url: "https://cdn.aframp.com/logos/9mobile.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "phone_number".to_string(),
                    field_label: "Phone Number".to_string(),
                    field_type: FieldType::Tel,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^(080|090|070|081)[0-9]{8}$".to_string()),
                        format: Some("080XXXXXXXX".to_string()),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("08012345678".to_string()),
                },
                RequiredField {
                    field_name: "plan_code".to_string(),
                    field_label: "Data Plan".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "9mobile-500mb".to_string(),
                            label: "500MB".to_string(),
                            amount: Some("500.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "9mobile-1gb".to_string(),
                            label: "1GB".to_string(),
                            amount: Some("1000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "9mobile-2gb".to_string(),
                            label: "2GB".to_string(),
                            amount: Some("2000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "9mobile-5gb".to_string(),
                            label: "5GB".to_string(),
                            amount: Some("5000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                        FieldOption {
                            value: "9mobile-10gb".to_string(),
                            label: "10GB".to_string(),
                            amount: Some("10000.00".to_string()),
                            description: Some("Valid for 30 days".to_string()),
                        },
                    ]),
                    placeholder: None,
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "500.00".to_string(),
                max_amount: "10000.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: true,
            },
            processing: ProcessingInfo {
                estimated_time: "Instant".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.0,
                },
            },
            help_url: Some("https://aframp.com/help/data".to_string()),
        },
        // ==================== CABLE TV PROVIDERS ====================
        BillProvider {
            provider_id: "dstv".to_string(),
            provider_code: "dstv-ng".to_string(),
            name: "DSTV".to_string(),
            short_name: "DSTV".to_string(),
            category: ProviderCategory::CableTv,
            description: "Pay for DSTV subscription".to_string(),
            logo_url: "https://cdn.aframp.com/logos/dstv.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "smart_card_number".to_string(),
                    field_label: "Smart Card Number".to_string(),
                    field_type: FieldType::Text,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^[0-9]{10}$".to_string()),
                        length: Some(10),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("10-digit smart card number".to_string()),
                },
                RequiredField {
                    field_name: "bouquet_code".to_string(),
                    field_label: "Subscription Package".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "dstv-padi".to_string(),
                            label: "DStv Padi".to_string(),
                            amount: Some("2500.00".to_string()),
                            description: Some("DStv Padi - ₦2,500/month".to_string()),
                        },
                        FieldOption {
                            value: "dstv-yanga".to_string(),
                            label: "DStv Yanga".to_string(),
                            amount: Some("3500.00".to_string()),
                            description: Some("DStv Yanga - ₦3,500/month".to_string()),
                        },
                        FieldOption {
                            value: "dstv-compact".to_string(),
                            label: "DStv Compact".to_string(),
                            amount: Some("7400.00".to_string()),
                            description: Some("DStv Compact - ₦7,400/month".to_string()),
                        },
                        FieldOption {
                            value: "dstv-compact-plus".to_string(),
                            label: "DStv Compact Plus".to_string(),
                            amount: Some("9300.00".to_string()),
                            description: Some("DStv Compact Plus - ₦9,300/month".to_string()),
                        },
                        FieldOption {
                            value: "dstv-premium".to_string(),
                            label: "DStv Premium".to_string(),
                            amount: Some("24500.00".to_string()),
                            description: Some("DStv Premium - ₦24,500/month".to_string()),
                        },
                    ]),
                    placeholder: None,
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "2500.00".to_string(),
                max_amount: "24500.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: true,
            },
            processing: ProcessingInfo {
                estimated_time: "Within 5 minutes".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.0,
                },
            },
            help_url: Some("https://aframp.com/help/dstv".to_string()),
        },
        BillProvider {
            provider_id: "gotv".to_string(),
            provider_code: "gotv-ng".to_string(),
            name: "GOtv".to_string(),
            short_name: "GOtv".to_string(),
            category: ProviderCategory::CableTv,
            description: "Pay for GOtv subscription".to_string(),
            logo_url: "https://cdn.aframp.com/logos/gotv.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "smart_card_number".to_string(),
                    field_label: "Smart Card Number".to_string(),
                    field_type: FieldType::Text,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^[0-9]{10}$".to_string()),
                        length: Some(10),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("10-digit smart card number".to_string()),
                },
                RequiredField {
                    field_name: "bouquet_code".to_string(),
                    field_label: "Subscription Package".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "gotv-jinja".to_string(),
                            label: "GOtv Jinja".to_string(),
                            amount: Some("4000.00".to_string()),
                            description: Some("GOtv Jinja - ₦4,000/month".to_string()),
                        },
                        FieldOption {
                            value: "gotv-lite".to_string(),
                            label: "GOtv Lite".to_string(),
                            amount: Some("2500.00".to_string()),
                            description: Some("GOtv Lite - ₦2,500/month".to_string()),
                        },
                        FieldOption {
                            value: "gotv-value".to_string(),
                            label: "GOtv Value".to_string(),
                            amount: Some("4800.00".to_string()),
                            description: Some("GOtv Value - ₦4,800/month".to_string()),
                        },
                        FieldOption {
                            value: "gotv-max".to_string(),
                            label: "GOtv Max".to_string(),
                            amount: Some("7200.00".to_string()),
                            description: Some("GOtv Max - ₦7,200/month".to_string()),
                        },
                    ]),
                    placeholder: None,
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "2500.00".to_string(),
                max_amount: "7200.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: true,
            },
            processing: ProcessingInfo {
                estimated_time: "Within 5 minutes".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.0,
                },
            },
            help_url: Some("https://aframp.com/help/gotv".to_string()),
        },
        BillProvider {
            provider_id: "startimes".to_string(),
            provider_code: "startimes-ng".to_string(),
            name: "Startimes".to_string(),
            short_name: "Startimes".to_string(),
            category: ProviderCategory::CableTv,
            description: "Pay for Startimes subscription".to_string(),
            logo_url: "https://cdn.aframp.com/logos/startimes.png".to_string(),
            status: ProviderStatus::Active,
            supported_currencies: vec!["cNGN".to_string()],
            required_fields: vec![
                RequiredField {
                    field_name: "smart_card_number".to_string(),
                    field_label: "Smart Card Number".to_string(),
                    field_type: FieldType::Text,
                    required: true,
                    validation: Some(FieldValidation {
                        pattern: Some("^[0-9]{11}$".to_string()),
                        length: Some(11),
                        ..Default::default()
                    }),
                    options: None,
                    placeholder: Some("11-digit smart card number".to_string()),
                },
                RequiredField {
                    field_name: "bouquet_code".to_string(),
                    field_label: "Subscription Package".to_string(),
                    field_type: FieldType::Select,
                    required: true,
                    validation: None,
                    options: Some(vec![
                        FieldOption {
                            value: "startimes-nova".to_string(),
                            label: "Nova".to_string(),
                            amount: Some("900.00".to_string()),
                            description: Some("Nova - ₦900/month".to_string()),
                        },
                        FieldOption {
                            value: "startimes-basic".to_string(),
                            label: "Basic".to_string(),
                            amount: Some("1900.00".to_string()),
                            description: Some("Basic - ₦1,900/month".to_string()),
                        },
                        FieldOption {
                            value: "startimes-classic".to_string(),
                            label: "Classic".to_string(),
                            amount: Some("2500.00".to_string()),
                            description: Some("Classic - ₦2,500/month".to_string()),
                        },
                        FieldOption {
                            value: "startimes-super".to_string(),
                            label: "Super".to_string(),
                            amount: Some("4200.00".to_string()),
                            description: Some("Super - ₦4,200/month".to_string()),
                        },
                    ]),
                    placeholder: None,
                },
            ],
            amount_limits: AmountLimits {
                min_amount: "900.00".to_string(),
                max_amount: "4200.00".to_string(),
                currency: "cNGN".to_string(),
                fixed_amounts: true,
            },
            processing: ProcessingInfo {
                estimated_time: "Within 5 minutes".to_string(),
                fees: ProcessingFees {
                    service_fee: "0.00".to_string(),
                    convenience_fee_percentage: 0.0,
                },
            },
            help_url: Some("https://aframp.com/help/startimes".to_string()),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_category_from_str() {
        assert_eq!(
            ProviderCategory::from_str("electricity"),
            Some(ProviderCategory::Electricity)
        );
        assert_eq!(
            ProviderCategory::from_str("airtime"),
            Some(ProviderCategory::Airtime)
        );
        assert_eq!(
            ProviderCategory::from_str("data"),
            Some(ProviderCategory::Data)
        );
        assert_eq!(
            ProviderCategory::from_str("cable_tv"),
            Some(ProviderCategory::CableTv)
        );
        assert_eq!(ProviderCategory::from_str("invalid"), None);
    }

    #[test]
    fn test_provider_category_to_string() {
        assert_eq!(ProviderCategory::Electricity.to_string(), "electricity");
        assert_eq!(ProviderCategory::Airtime.to_string(), "airtime");
        assert_eq!(ProviderCategory::Data.to_string(), "data");
        assert_eq!(ProviderCategory::CableTv.to_string(), "cable_tv");
    }

    #[test]
    fn test_get_all_providers() {
        let providers = get_all_providers();
        assert!(!providers.is_empty());

        // Check we have all categories
        let electricity_count = providers
            .iter()
            .filter(|p| p.category == ProviderCategory::Electricity)
            .count();
        let airtime_count = providers
            .iter()
            .filter(|p| p.category == ProviderCategory::Airtime)
            .count();
        let data_count = providers
            .iter()
            .filter(|p| p.category == ProviderCategory::Data)
            .count();
        let cable_count = providers
            .iter()
            .filter(|p| p.category == ProviderCategory::CableTv)
            .count();

        assert_eq!(electricity_count, 8); // 8 DISCOs
        assert_eq!(airtime_count, 4); // 4 networks
        assert_eq!(data_count, 4); // 4 networks
        assert_eq!(cable_count, 3); // DSTV, GOtv, Startimes

        assert_eq!(providers.len(), 19); // Total (8 electricity + 4 airtime + 4 data + 3 cable)
    }

    #[test]
    fn test_build_category_summary() {
        let providers = get_all_providers();
        let categories = build_category_summary(&providers);

        assert_eq!(categories.len(), 4);

        let electricity_cat = categories
            .iter()
            .find(|c| c.category_id == "electricity")
            .unwrap();
        assert_eq!(electricity_cat.count, 8);

        let airtime_cat = categories
            .iter()
            .find(|c| c.category_id == "airtime")
            .unwrap();
        assert_eq!(airtime_cat.count, 4);

        let data_cat = categories.iter().find(|c| c.category_id == "data").unwrap();
        assert_eq!(data_cat.count, 4);

        let cable_cat = categories
            .iter()
            .find(|c| c.category_id == "cable_tv")
            .unwrap();
        assert_eq!(cable_cat.count, 3);
    }
}
