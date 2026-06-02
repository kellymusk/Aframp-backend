//! Automated Regulatory Report Generation & Compliance Filings Pipeline — Issue #523
//!
//! Generates CTR, SAR, LIQUIDITY_RATIO, and CROSS_BORDER_FLOW reports,
//! transmits them to central bank gateways, and tracks ACK/NACK responses.

pub mod handlers;
pub mod models;
pub mod repository;
pub mod routes;
pub mod service;

pub use models::{AgencyGateway, AuditFilingEvent, RegulatoryReport, ReportStatus, ReportType};
pub use service::RegulatoryFilingService;
