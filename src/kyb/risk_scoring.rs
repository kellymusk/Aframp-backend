//! KYB Risk Scoring Engine
//!
//! Scores a business entity 0–100 based on industry, jurisdiction, and registry status.
//! Score >= 60 → Enhanced Due Diligence; < 60 → Light Due Diligence.

use super::models::RegistryEntityData;
use serde_json::json;

pub const ENHANCED_THRESHOLD: f64 = 60.0;

/// Risk factors contributing to the final score.
#[derive(Debug)]
pub struct RiskFactors {
    pub industry_risk: f64,
    pub jurisdiction_risk: f64,
    pub registry_status_risk: f64,
    pub ubo_count_risk: f64,
}

impl RiskFactors {
    pub fn total(&self) -> f64 {
        (self.industry_risk + self.jurisdiction_risk + self.registry_status_risk + self.ubo_count_risk)
            .min(100.0)
    }

    pub fn risk_level(&self) -> &'static str {
        if self.total() >= ENHANCED_THRESHOLD { "enhanced" } else { "light" }
    }

    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "industry_risk": self.industry_risk,
            "jurisdiction_risk": self.jurisdiction_risk,
            "registry_status_risk": self.registry_status_risk,
            "ubo_count_risk": self.ubo_count_risk,
            "total": self.total(),
            "risk_level": self.risk_level()
        })
    }
}

pub struct RiskScoringEngine;

impl RiskScoringEngine {
    /// Score a business based on available data.
    pub fn score(
        jurisdiction: &str,
        industry_code: Option<&str>,
        registry_data: Option<&RegistryEntityData>,
    ) -> RiskFactors {
        let industry_risk = industry_risk_score(industry_code);
        let jurisdiction_risk = jurisdiction_risk_score(jurisdiction);
        let registry_status_risk = registry_data
            .map(|r| registry_status_risk(&r.status))
            .unwrap_or(20.0); // Unknown = moderate risk
        let ubo_count_risk = registry_data
            .map(|r| ubo_count_risk(r.shareholders.iter().filter(|s| s.ownership_percentage >= 25.0).count()))
            .unwrap_or(0.0);

        RiskFactors { industry_risk, jurisdiction_risk, registry_status_risk, ubo_count_risk }
    }
}

// ── Scoring Tables ────────────────────────────────────────────────────────────

fn industry_risk_score(code: Option<&str>) -> f64 {
    match code.unwrap_or("") {
        // High-risk industries
        c if c.starts_with("52") => 35.0, // Finance / Money Services
        c if c.starts_with("71") => 30.0, // Arts / Gambling
        c if c.starts_with("92") => 25.0, // Public Administration
        c if c.starts_with("44") || c.starts_with("45") => 15.0, // Retail
        c if c.starts_with("72") => 20.0, // Accommodation / Food
        _ => 10.0, // Default / unknown
    }
}

fn jurisdiction_risk_score(jurisdiction: &str) -> f64 {
    match jurisdiction.to_uppercase().as_str() {
        // FATF Black List
        "KP" | "IR" => 40.0,
        // FATF Grey List
        "MM" | "PK" | "SY" | "YE" | "SD" => 30.0,
        // Moderate risk
        "NG" | "KE" | "GH" => 15.0,
        // Lower risk
        "GB" | "US" | "DE" | "FR" | "ZA" => 5.0,
        _ => 20.0,
    }
}

fn registry_status_risk(status: &str) -> f64 {
    match status {
        "active" => 0.0,
        "inactive" => 50.0,
        "deregistered" => 80.0,
        _ => 20.0,
    }
}

fn ubo_count_risk(ubo_count: usize) -> f64 {
    match ubo_count {
        0 => 15.0, // No identifiable UBOs = opacity risk
        1 => 5.0,
        2..=3 => 0.0,
        _ => 10.0, // Many UBOs = complex structure
    }
}
