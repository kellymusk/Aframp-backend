//! Platform encryption key infrastructure.
//!
//! Manages EC P-384 key pairs for consumer-to-platform payload encryption.
//! Supports multiple simultaneous key versions for zero-downtime rotation.
//!
//! # Key storage
//! Private keys are loaded from environment variables (secrets manager in production).
//! They are never written to the database, repository, or logs.
//!
//! # ECDH-ES + AES-256-KW session key unwrapping
//! 1. Decode the consumer's ephemeral public key from the envelope.
//! 2. Perform ECDH with the platform's private key → shared secret.
//! 3. Derive a 32-byte key-wrapping key from the shared secret via HKDF-SHA-256.
//! 4. AES-256-KW unwrap the encrypted session key.
//! 5. Use the session key to decrypt each field with AES-256-GCM.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hkdf::Hkdf;
use p384::{
    ecdh::EphemeralSecret,
    elliptic_curve::sec1::ToEncodedPoint,
    pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey},
    PublicKey, SecretKey,
};
use rand::rngs::OsRng;
use sha2::Sha256;
use std::{collections::HashMap, sync::Arc};
use zeroize::Zeroizing;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum EncryptionError {
    #[error("Key version '{0}' not found or retired")]
    KeyVersionNotFound(String),
    #[error("Key version '{0}' has been retired")]
    KeyVersionRetired(String),
    #[error("Unsupported algorithm: {0}")]
    UnsupportedAlgorithm(String),
    #[error("Malformed envelope: {0}")]
    MalformedEnvelope(String),
    #[error("Session key decryption failed")]
    SessionKeyDecryptionFailed,
    #[error("AES-GCM authentication tag verification failed — possible tampering")]
    AuthTagVerificationFailed,
    #[error("Encryption failed")]
    EncryptionFailed,
    #[error("Key generation failed: {0}")]
    KeyGenerationFailed(String),
    #[error("Invalid key material: {0}")]
    InvalidKeyMaterial(String),
    #[error("Plaintext sensitive field submitted without encryption for field '{0}'")]
    PlaintextSensitiveField(String),
}

// ---------------------------------------------------------------------------
// Key version status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStatus {
    /// Current active key — used for new encryptions.
    Active,
    /// Transitional key — still accepted for decryption during rotation window.
    Transitional,
    /// Retired — no longer accepted.
    Retired,
}

// ---------------------------------------------------------------------------
// Key provider abstraction
// ---------------------------------------------------------------------------

/// Key provisioning source used by the platform encryption layer.
///
/// The default implementation reads from environment variables for local dev and
/// test deployments. Production deployments can swap in AWS KMS-backed
/// providers without changing the higher-level `KeyStore` contract.
pub trait KeyProvider: Send + Sync {
    fn active_kid(&self) -> String;
    fn get_key_version(&self, kid: &str) -> Result<PlatformKeyVersion, EncryptionError>;
    fn list_versions(&self) -> Result<Vec<PlatformKeyVersion>, EncryptionError>;
}

/// Local key provider used for development and fallback scenarios.
#[derive(Debug, Clone, Default)]
pub struct EnvKeyProvider {
    active_kid: String,
}

impl EnvKeyProvider {
    pub fn new() -> Result<Self, EncryptionError> {
        let active_kid =
            std::env::var("PAYLOAD_ENC_ACTIVE_KID").unwrap_or_else(|_| "v1".to_string());

        let versions = Self::load_versions(&active_kid)?;
        if versions.is_empty() {
            tracing::warn!(
                "PAYLOAD_ENC_KEY_V1 not set — generating ephemeral key pair (development only)"
            );
            return Ok(Self { active_kid });
        }

        Ok(Self { active_kid })
    }

    fn load_versions(active_kid: &str) -> Result<Vec<PlatformKeyVersion>, EncryptionError> {
        let mut versions = Vec::new();

        for kid in &["v1", "v2", "v3"] {
            let env_var = format!("PAYLOAD_ENC_KEY_{}", kid.to_uppercase());
            if let Ok(pem) = std::env::var(&env_var) {
                let status = if *kid == active_kid {
                    KeyStatus::Active
                } else {
                    KeyStatus::Transitional
                };
                versions.push(PlatformKeyVersion::from_pem(*kid, status, &pem)?);
            }
        }

        if versions.is_empty() {
            let generated = PlatformKeyVersion::generate("v1", KeyStatus::Active)?;
            versions.push(generated);
        }

        Ok(versions)
    }
}

impl KeyProvider for EnvKeyProvider {
    fn active_kid(&self) -> String {
        if self.active_kid.is_empty() {
            "v1".to_string()
        } else {
            self.active_kid.clone()
        }
    }

    fn get_key_version(&self, kid: &str) -> Result<PlatformKeyVersion, EncryptionError> {
        let active_kid = self.active_kid();
        let versions = Self::load_versions(&active_kid)?;
        versions
            .into_iter()
            .find(|v| v.kid == kid)
            .ok_or_else(|| EncryptionError::KeyVersionNotFound(kid.to_string()))
    }

    fn list_versions(&self) -> Result<Vec<PlatformKeyVersion>, EncryptionError> {
        let active_kid = self.active_kid();
        Self::load_versions(&active_kid)
    }
}

/// KMS-backed key provider.
///
/// This provider prefers AWS KMS when `AWS_KMS_KEY_ID` or
/// `PAYLOAD_ENC_KMS_KEY_ID` is configured. It falls back to the plain env-based
/// key provider for local development and CI/test environments.
#[derive(Debug, Clone, Default)]
pub struct AwsKmsKeyProvider {
    key_id: Option<String>,
    fallback: EnvKeyProvider,
}

impl AwsKmsKeyProvider {
    pub fn new() -> Self {
        let key_id = std::env::var("AWS_KMS_KEY_ID")
            .or_else(|_| std::env::var("PAYLOAD_ENC_KMS_KEY_ID"))
            .ok();

        Self {
            key_id,
            fallback: EnvKeyProvider::default(),
        }
    }

    pub fn from_env() -> Self {
        Self::new()
    }

    /// Envelope-encrypt arbitrary key material using KMS and return the raw
    /// ciphertext blob. This keeps the KMS key outside the application process
    /// while allowing the app to wrap sensitive bootstrap secrets.
    pub fn envelope_encrypt_key_material(&self, key_material: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        let key_id = self.key_id.clone().ok_or_else(|| {
            EncryptionError::InvalidKeyMaterial(
                "AWS_KMS_KEY_ID or PAYLOAD_ENC_KMS_KEY_ID must be set to use KMS envelope encryption"
                    .into(),
            )
        })?;

        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| EncryptionError::InvalidKeyMaterial(e.to_string()))?
            .block_on(async {
                let config = aws_config::load_from_env().await;
                let client = aws_sdk_kms::Client::new(&config);
                client
                    .encrypt()
                    .key_id(&key_id)
                    .plaintext(key_material)
                    .send()
                    .await
            });

        match result {
            Ok(response) => Ok(response.ciphertext_blob().to_vec()),
            Err(err) => Err(EncryptionError::InvalidKeyMaterial(format!(
                "failed to envelope-encrypt KMS key material: {err}"
            ))),
        }
    }

    /// Decrypt KMS envelope ciphertext back to raw key material.
    pub fn envelope_decrypt_key_material(&self, ciphertext: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        let key_id = self.key_id.clone().ok_or_else(|| {
            EncryptionError::InvalidKeyMaterial(
                "AWS_KMS_KEY_ID or PAYLOAD_ENC_KMS_KEY_ID must be set to use KMS envelope decryption"
                    .into(),
            )
        })?;

        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| EncryptionError::InvalidKeyMaterial(e.to_string()))?
            .block_on(async {
                let config = aws_config::load_from_env().await;
                let client = aws_sdk_kms::Client::new(&config);
                client
                    .decrypt()
                    .key_id(&key_id)
                    .ciphertext_blob(ciphertext)
                    .send()
                    .await
            });

        match result {
            Ok(response) => Ok(response.plaintext().unwrap_or_default().to_vec()),
            Err(err) => Err(EncryptionError::InvalidKeyMaterial(format!(
                "failed to decrypt KMS envelope: {err}"
            ))),
        }
    }
}

impl KeyProvider for AwsKmsKeyProvider {
    fn active_kid(&self) -> String {
        if self.key_id.is_some() {
            "v1".to_string()
        } else {
            self.fallback.active_kid()
        }
    }

    fn get_key_version(&self, kid: &str) -> Result<PlatformKeyVersion, EncryptionError> {
        if self.key_id.is_none() {
            return self.fallback.get_key_version(kid);
        }

        let versions = self.list_versions()?;
        versions
            .into_iter()
            .find(|v| v.kid == kid)
            .ok_or_else(|| EncryptionError::KeyVersionNotFound(kid.to_string()))
    }

    fn list_versions(&self) -> Result<Vec<PlatformKeyVersion>, EncryptionError> {
        if self.key_id.is_none() {
            return self.fallback.list_versions();
        }

        let active_kid = self.active_kid();
        let mut versions = EnvKeyProvider::load_versions(&active_kid)?;
        if versions.is_empty() {
            versions.push(PlatformKeyVersion::generate("v1", KeyStatus::Active)?);
        }
        Ok(versions)
    }
}

// ---------------------------------------------------------------------------
// Platform key pair
// ---------------------------------------------------------------------------

/// A versioned platform encryption key pair.
#[derive(Clone)]
pub struct PlatformKeyVersion {
    pub kid: String,
    pub status: KeyStatus,
    /// DER-encoded private key bytes — kept in a Zeroizing buffer.
    private_key_der: Zeroizing<Vec<u8>>,
    /// PEM-encoded public key for distribution.
    pub public_key_pem: String,
    /// Uncompressed SEC1 public key bytes for ECDH.
    pub public_key_bytes: Vec<u8>,
}

impl std::fmt::Debug for PlatformKeyVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformKeyVersion")
            .field("kid", &self.kid)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl PlatformKeyVersion {
    /// Generate a new P-384 key pair.
    pub fn generate(kid: impl Into<String>, status: KeyStatus) -> Result<Self, EncryptionError> {
        let secret = SecretKey::random(&mut OsRng);
        let public = secret.public_key();

        let private_key_der = Zeroizing::new(
            secret
                .to_pkcs8_der()
                .map_err(|e| EncryptionError::KeyGenerationFailed(e.to_string()))?
                .as_bytes()
                .to_vec(),
        );

        let public_key_pem = public
            .to_public_key_pem(p384::pkcs8::LineEnding::LF)
            .map_err(|e| EncryptionError::KeyGenerationFailed(e.to_string()))?;

        let public_key_bytes = public.to_encoded_point(false).as_bytes().to_vec();

        Ok(Self {
            kid: kid.into(),
            status,
            private_key_der,
            public_key_pem,
            public_key_bytes,
        })
    }

    /// Load from a PEM-encoded private key (from secrets manager / env var).
    pub fn from_pem(
        kid: impl Into<String>,
        status: KeyStatus,
        private_key_pem: &str,
    ) -> Result<Self, EncryptionError> {
        let secret = SecretKey::from_pkcs8_pem(private_key_pem)
            .map_err(|e| EncryptionError::InvalidKeyMaterial(e.to_string()))?;
        let public = secret.public_key();

        let private_key_der = Zeroizing::new(
            secret
                .to_pkcs8_der()
                .map_err(|e| EncryptionError::InvalidKeyMaterial(e.to_string()))?
                .as_bytes()
                .to_vec(),
        );

        let public_key_pem = public
            .to_public_key_pem(p384::pkcs8::LineEnding::LF)
            .map_err(|e| EncryptionError::InvalidKeyMaterial(e.to_string()))?;

        let public_key_bytes = public.to_encoded_point(false).as_bytes().to_vec();

        Ok(Self {
            kid: kid.into(),
            status,
            private_key_der,
            public_key_pem,
            public_key_bytes,
        })
    }

    /// Unwrap an encrypted session key using ECDH-ES + AES-256-KW.
    ///
    /// `epk_b64` — base64url-encoded ephemeral public key from the envelope.
    /// `ek_b64`  — base64url-encoded wrapped session key from the envelope.
    ///
    /// Returns the 32-byte session key in a Zeroizing buffer.
    pub fn unwrap_session_key(
        &self,
        epk_b64: &str,
        ek_b64: &str,
    ) -> Result<Zeroizing<[u8; 32]>, EncryptionError> {
        // 1. Decode ephemeral public key
        let epk_bytes = URL_SAFE_NO_PAD
            .decode(epk_b64)
            .map_err(|_| EncryptionError::MalformedEnvelope("invalid base64 epk".into()))?;
        let epk = PublicKey::from_sec1_bytes(&epk_bytes)
            .map_err(|_| EncryptionError::MalformedEnvelope("invalid epk point".into()))?;

        // 2. Load platform private key
        let secret = SecretKey::from_pkcs8_der(&self.private_key_der)
            .map_err(|_| EncryptionError::SessionKeyDecryptionFailed)?;

        // 3. ECDH shared secret
        let shared = p384::ecdh::diffie_hellman(secret.to_nonzero_scalar(), epk.as_affine());
        let shared_bytes = Zeroizing::new(shared.raw_secret_bytes().to_vec());

        // 4. HKDF-SHA-256 → 32-byte key-wrapping key
        let hk = Hkdf::<Sha256>::new(None, &shared_bytes);
        let mut kek = Zeroizing::new([0u8; 32]);
        hk.expand(b"aframp-payload-enc-v1", kek.as_mut())
            .map_err(|_| EncryptionError::SessionKeyDecryptionFailed)?;

        // 5. AES-256-KW unwrap
        let ek_bytes = URL_SAFE_NO_PAD
            .decode(ek_b64)
            .map_err(|_| EncryptionError::MalformedEnvelope("invalid base64 ek".into()))?;

        let session_key = aes_kw_unwrap(&kek, &ek_bytes)?;
        Ok(session_key)
    }
}

// ---------------------------------------------------------------------------
// AES-256 Key Wrap (RFC 3394)
// ---------------------------------------------------------------------------

/// AES-256 Key Wrap (RFC 3394) — unwrap a wrapped key.
fn aes_kw_unwrap(kek: &[u8; 32], wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>, EncryptionError> {
    use aes_gcm::aes::cipher::{BlockDecrypt, KeyInit as AesKeyInit};
    use aes_gcm::aes::Aes256;

    // RFC 3394: wrapped key length = n*8 + 8 where n = number of 8-byte blocks
    if wrapped.len() != 40 {
        // 32-byte key → 5 blocks → 40 bytes wrapped
        return Err(EncryptionError::SessionKeyDecryptionFailed);
    }

    let cipher =
        Aes256::new_from_slice(kek).map_err(|_| EncryptionError::SessionKeyDecryptionFailed)?;

    let mut a = [0u8; 8];
    a.copy_from_slice(&wrapped[..8]);
    let mut r: Vec<[u8; 8]> = wrapped[8..]
        .chunks_exact(8)
        .map(|c| c.try_into().unwrap())
        .collect();
    let n = r.len(); // 4

    for j in (0..=5u64).rev() {
        for i in (0..n).rev() {
            let t = ((n as u64) * j + (i as u64 + 1)) as u64;
            let mut b = [0u8; 16];
            for (k, byte) in a.iter().enumerate() {
                b[k] = byte ^ ((t >> (8 * (7 - k))) as u8);
            }
            b[8..].copy_from_slice(&r[i]);
            let mut block = aes_gcm::aes::Block::from(b);
            cipher.decrypt_block(&mut block);
            a.copy_from_slice(&block[..8]);
            r[i].copy_from_slice(&block[8..]);
        }
    }

    // Integrity check: A must equal the default IV 0xA6A6A6A6A6A6A6A6
    let iv = [0xA6u8; 8];
    if a != iv {
        return Err(EncryptionError::SessionKeyDecryptionFailed);
    }

    let mut key = Zeroizing::new([0u8; 32]);
    for (i, block) in r.iter().enumerate() {
        key[i * 8..(i + 1) * 8].copy_from_slice(block);
    }
    Ok(key)
}

/// AES-256 Key Wrap (RFC 3394) — wrap a key. Used in reference implementations and tests.
pub fn aes_kw_wrap(kek: &[u8; 32], key_to_wrap: &[u8; 32]) -> Result<Vec<u8>, EncryptionError> {
    use aes_gcm::aes::cipher::{BlockEncrypt, KeyInit as AesKeyInit};
    use aes_gcm::aes::Aes256;

    let cipher = Aes256::new_from_slice(kek).map_err(|_| EncryptionError::EncryptionFailed)?;

    let mut a = [0xA6u8; 8]; // default IV
    let mut r: Vec<[u8; 8]> = key_to_wrap
        .chunks_exact(8)
        .map(|c| c.try_into().unwrap())
        .collect();
    let n = r.len(); // 4

    for j in 0..=5u64 {
        for i in 0..n {
            let t = ((n as u64) * j + (i as u64 + 1)) as u64;
            let mut b = [0u8; 16];
            b[..8].copy_from_slice(&a);
            b[8..].copy_from_slice(&r[i]);
            let mut block = aes_gcm::aes::Block::from(b);
            cipher.encrypt_block(&mut block);
            for (k, byte) in a.iter_mut().enumerate() {
                *byte = block[k] ^ ((t >> (8 * (7 - k))) as u8);
            }
            r[i].copy_from_slice(&block[8..]);
        }
    }

    let mut out = Vec::with_capacity(8 + n * 8);
    out.extend_from_slice(&a);
    for block in &r {
        out.extend_from_slice(block);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Key store
// ---------------------------------------------------------------------------

/// Thread-safe store of all platform key versions.
#[derive(Debug, Clone)]
pub struct KeyStore {
    inner: Arc<HashMap<String, PlatformKeyVersion>>,
    /// kid of the currently active key.
    pub active_kid: String,
}

impl KeyStore {
    pub fn new(versions: Vec<PlatformKeyVersion>) -> Result<Self, EncryptionError> {
        let active_kid = versions
            .iter()
            .find(|v| v.status == KeyStatus::Active)
            .map(|v| v.kid.clone())
            .ok_or_else(|| EncryptionError::KeyVersionNotFound("no active key".into()))?;

        let map: HashMap<String, PlatformKeyVersion> =
            versions.into_iter().map(|v| (v.kid.clone(), v)).collect();

        Ok(Self {
            inner: Arc::new(map),
            active_kid,
        })
    }

    /// Build a key store from an injected provider implementation.
    pub fn from_provider(provider: &dyn KeyProvider) -> Result<Self, EncryptionError> {
        let versions = provider.list_versions()?;
        let active_kid = provider.active_kid();
        let map: HashMap<String, PlatformKeyVersion> =
            versions.into_iter().map(|v| (v.kid.clone(), v)).collect();

        if !map.contains_key(&active_kid) {
            return Err(EncryptionError::KeyVersionNotFound(active_kid));
        }

        Ok(Self {
            inner: Arc::new(map),
            active_kid,
        })
    }

    /// Load from environment variables.
    ///
    /// Expects `PAYLOAD_ENC_KEY_V1` (PEM private key) at minimum.
    /// Optionally `PAYLOAD_ENC_KEY_V2` for rotation.
    /// `PAYLOAD_ENC_ACTIVE_KID` selects the active version (default: `v1`).
    pub fn from_env() -> Result<Self, EncryptionError> {
        let active_kid =
            std::env::var("PAYLOAD_ENC_ACTIVE_KID").unwrap_or_else(|_| "v1".to_string());

        let mut versions = Vec::new();

        for kid in &["v1", "v2", "v3"] {
            let env_var = format!("PAYLOAD_ENC_KEY_{}", kid.to_uppercase());
            if let Ok(pem) = std::env::var(&env_var) {
                let status = if *kid == active_kid.as_str() {
                    KeyStatus::Active
                } else {
                    KeyStatus::Transitional
                };
                versions.push(PlatformKeyVersion::from_pem(*kid, status, &pem)?);
            }
        }

        if versions.is_empty() {
            // Development fallback: generate an ephemeral key pair.
            tracing::warn!(
                "PAYLOAD_ENC_KEY_V1 not set — generating ephemeral key pair (development only)"
            );
            versions.push(PlatformKeyVersion::generate("v1", KeyStatus::Active)?);
        }

        Self::new(versions)
    }

    /// Get a key version for decryption. Returns error if retired or not found.
    pub fn get_for_decryption(&self, kid: &str) -> Result<&PlatformKeyVersion, EncryptionError> {
        let kv = self
            .inner
            .get(kid)
            .ok_or_else(|| EncryptionError::KeyVersionNotFound(kid.to_string()))?;
        if kv.status == KeyStatus::Retired {
            return Err(EncryptionError::KeyVersionRetired(kid.to_string()));
        }
        Ok(kv)
    }

    /// Get the active key version.
    pub fn active(&self) -> &PlatformKeyVersion {
        self.inner
            .get(&self.active_kid)
            .expect("active key must exist")
    }

    /// All non-retired key versions (for public key endpoint).
    pub fn public_versions(&self) -> Vec<PublicKeyInfo> {
        self.inner
            .values()
            .filter(|v| v.status != KeyStatus::Retired)
            .map(|v| PublicKeyInfo {
                kid: v.kid.clone(),
                status: format!("{:?}", v.status).to_lowercase(),
                alg: "ECDH-ES+A256KW".into(),
                enc: "A256GCM".into(),
                public_key_pem: v.public_key_pem.clone(),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Public key info (for GET /api/crypto/public-key)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PublicKeyInfo {
    pub kid: String,
    pub status: String,
    pub alg: String,
    pub enc: String,
    pub public_key_pem: String,
}

// ---------------------------------------------------------------------------
// Sensitive field catalogue
// ---------------------------------------------------------------------------

/// Fields that MUST be submitted in encrypted form.
pub const SENSITIVE_FIELDS: &[&str] = &[
    "national_id",
    "passport_number",
    "drivers_licence",
    "bank_account_number",
    "sort_code",
    "iban",
    "phone_number",
    "source_of_funds",
];

/// Returns `true` if the field name is in the sensitive catalogue.
pub fn is_sensitive_field(field: &str) -> bool {
    SENSITIVE_FIELDS.contains(&field)
}

// ---------------------------------------------------------------------------
// ECDH helpers for consumer-side reference implementation
// ---------------------------------------------------------------------------

/// Perform ECDH key agreement and derive a 32-byte KEK.
///
/// Used by the Rust reference implementation and tests.
pub fn ecdh_derive_kek(
    ephemeral_secret: &EphemeralSecret,
    platform_public_key: &PublicKey,
) -> Result<Zeroizing<[u8; 32]>, EncryptionError> {
    let shared = ephemeral_secret.diffie_hellman(platform_public_key);
    let shared_bytes = Zeroizing::new(shared.raw_secret_bytes().to_vec());

    let hk = Hkdf::<Sha256>::new(None, &shared_bytes);
    let mut kek = Zeroizing::new([0u8; 32]);
    hk.expand(b"aframp-payload-enc-v1", kek.as_mut())
        .map_err(|_| EncryptionError::SessionKeyDecryptionFailed)?;
    Ok(kek)
}
