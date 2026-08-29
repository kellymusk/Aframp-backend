//! UBO (Ultimate Beneficial Owner) Extraction Service
//!
//! Identifies individuals owning >= 25% of a business from registry data
//! and triggers individual KYC checks for each UBO.

use super::models::{RegistryEntityData, Ubo};
use super::repository::KybRepository;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

pub const UBO_THRESHOLD: f64 = 25.0;

pub struct UboService {
    repo: Arc<KybRepository>,
}

impl UboService {
    pub fn new(repo: Arc<KybRepository>) -> Self {
        Self { repo }
    }

    /// Extract UBOs from registry data, persist them, and trigger KYC for each.
    pub async fn extract_and_trigger(
        &self,
        kyb_application_id: Uuid,
        registry_data: &RegistryEntityData,
    ) -> Result<Vec<Ubo>, anyhow::Error> {
        let qualifying: Vec<_> = registry_data
            .shareholders
            .iter()
            .filter(|s| s.ownership_percentage >= UBO_THRESHOLD)
            .collect();

        info!(
            kyb_id = %kyb_application_id,
            ubo_count = qualifying.len(),
            "Extracted UBOs from registry data"
        );

        let mut ubos = Vec::new();
        for shareholder in qualifying {
            let ubo = self
                .repo
                .upsert_ubo(kyb_application_id, &shareholder.name, shareholder.ownership_percentage)
                .await?;

            // Trigger individual KYC (fire-and-forget — KYC service handles async)
            self.trigger_kyc_for_ubo(&ubo).await;

            ubos.push(ubo);
        }

        Ok(ubos)
    }

    /// Enqueue a KYC check for a UBO. In production this would call the KYC service.
    async fn trigger_kyc_for_ubo(&self, ubo: &Ubo) {
        info!(
            ubo_id = %ubo.id,
            name = %ubo.full_name,
            ownership = %ubo.ownership_percentage,
            "Triggering individual KYC for UBO"
        );
        // KYC trigger is async — the KYC service will update kyb_ubos.kyc_status
        // via a webhook or background job once verification completes.
        // Mark as pending immediately.
        let _ = self.repo.set_ubo_kyc_status(ubo.id, "pending").await;
    }
}
