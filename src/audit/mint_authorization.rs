use crate::audit::redaction::{compute_entry_hash, sha256_hex};
use crate::database::error::DatabaseError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "mint_action_type", rename_all = "snake_case")]
pub enum MintActionType {
    MintRequested,
    MintApproved,
    MintSubmitted,
    MintCompleted,
    MintFailed,
}

impl MintActionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MintRequested => "mint_requested",
            Self::MintApproved => "mint_approved",
            Self::MintSubmitted => "mint_submitted",
            Self::MintCompleted => "mint_completed",
            Self::MintFailed => "mint_failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintAuthorizationLogEntry {
    pub id: Uuid,
    pub actor_id: String,
    pub public_key: String,
    pub action_type: MintActionType,
    pub request_payload: JsonValue,
    pub previous_hash: String,
    pub current_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MintAuthorizationChainVerificationResult {
    pub valid: bool,
    pub total_checked: i64,
    pub first_sequence_id: Option<Uuid>,
    pub last_sequence_id: Option<Uuid>,
    pub tampered_entries: Vec<TamperedMintAuthorizationEntry>,
    pub gaps_detected: Vec<String>,
    pub verified_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TamperedMintAuthorizationEntry {
    pub entry_id: Uuid,
    pub expected_hash: String,
    pub actual_hash: String,
    pub created_at: DateTime<Utc>,
}

pub struct MintAuthorizationRepository {
    pool: PgPool,
}

impl MintAuthorizationRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn last_entry_hash(&self) -> Result<Option<String>, DatabaseError> {
        let row = sqlx::query_scalar!(
            "SELECT current_hash FROM mint_authorization_logs ORDER BY created_at DESC LIMIT 1"
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)?;
        Ok(row)
    }

    pub async fn insert(&self, entry: &MintAuthorizationLogEntry) -> Result<(), DatabaseError> {
        sqlx::query!(
            "INSERT INTO mint_authorization_logs
             (id, actor_id, public_key, action_type, request_payload, previous_hash, current_hash, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            entry.id,
            entry.actor_id,
            entry.public_key,
            entry.action_type.as_str(),
            entry.request_payload,
            entry.previous_hash,
            entry.current_hash,
            entry.created_at,
        )
        .execute(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)?;

        Ok(())
    }

    pub async fn verify_hash_chain(
        &self,
        date_from: DateTime<Utc>,
        date_to: DateTime<Utc>,
    ) -> Result<MintAuthorizationChainVerificationResult, DatabaseError> {
        let rows = sqlx::query!(
            "SELECT id, actor_id, public_key, action_type as \"action_type: MintActionType\", request_payload, previous_hash, current_hash, created_at
             FROM mint_authorization_logs
             WHERE created_at >= $1 AND created_at <= $2
             ORDER BY created_at ASC",
            date_from,
            date_to,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(DatabaseError::from_sqlx)?;

        let total = rows.len() as i64;
        let mut tampered = Vec::new();
        let mut gaps = Vec::new();
        let mut prev_hash = None;

        const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

        let first_id = rows.first().map(|r| r.id);
        let last_id = rows.last().map(|r| r.id);

        for row in rows {
            let action_type = row.action_type;
            let content = format!(
                "{}|{}|{}|{}|{}|{}",
                row.id,
                row.actor_id,
                row.public_key,
                action_type.as_str(),
                row.request_payload.to_string(),
                row.created_at.timestamp_millis()
            );

            let previous = prev_hash.as_deref().unwrap_or(GENESIS_HASH);
            let expected = compute_entry_hash(previous, &content);

            if expected != row.current_hash {
                tampered.push(TamperedMintAuthorizationEntry {
                    entry_id: row.id,
                    expected_hash: expected.clone(),
                    actual_hash: row.current_hash.clone(),
                    created_at: row.created_at,
                });
            }

            if let Some(stored_prev) = row.previous_hash.as_ref() {
                if let Some(actual_prev) = prev_hash.as_ref() {
                    if stored_prev != actual_prev {
                        gaps.push(format!(
                            "Hash chain gap at entry {} (created_at: {})",
                            row.id, row.created_at
                        ));
                    }
                }
            }

            prev_hash = Some(row.current_hash);
        }

        Ok(MintAuthorizationChainVerificationResult {
            valid: tampered.is_empty() && gaps.is_empty(),
            total_checked: total,
            first_sequence_id: first_id,
            last_sequence_id: last_id,
            tampered_entries: tampered,
            gaps_detected: gaps,
            verified_at: Utc::now(),
        })
    }
}

pub struct MintAuthorizationService {
    repo: MintAuthorizationRepository,
}

impl MintAuthorizationService {
    pub fn new(repo: MintAuthorizationRepository) -> Self {
        Self { repo }
    }

    pub async fn record_event(
        &self,
        actor_id: &str,
        public_key: &str,
        action_type: MintActionType,
        request_payload: JsonValue,
    ) -> Result<(), String> {
        let previous_hash = self
            .repo
            .last_entry_hash()
            .await
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|| "0".repeat(64));

        let timestamp = Utc::now();
        let content = format!(
            "{}|{}|{}|{}|{}|{}|{}",
            actor_id,
            public_key,
            action_type.as_str(),
            request_payload.to_string(),
            previous_hash,
            timestamp.timestamp_millis(),
            "mint_audit"
        );

        let current_hash = compute_entry_hash(&previous_hash, &content);

        let entry = MintAuthorizationLogEntry {
            id: Uuid::new_v4(),
            actor_id: actor_id.to_string(),
            public_key: public_key.to_string(),
            action_type,
            request_payload,
            previous_hash,
            current_hash,
            created_at: timestamp,
        };

        self.repo
            .insert(&entry)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_action_type_as_str() {
        assert_eq!(MintActionType::MintRequested.as_str(), "mint_requested");
        assert_eq!(MintActionType::MintCompleted.as_str(), "mint_completed");
    }

    #[test]
    fn compute_hash_chain_deterministic() {
        let base = "0".repeat(64);
        let c1 = "content1";
        let h1 = compute_entry_hash(&base, c1);
        let h2 = compute_entry_hash(&h1, "content2");
        assert_ne!(h1, h2);
        assert_eq!(h2, compute_entry_hash(&h1, "content2"));
        assert_eq!(h1.len(), 64);
        assert_eq!(h2.len(), 64);
    }
}
