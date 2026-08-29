//! KYB Document Storage Service
//!
//! Encrypts business documents at rest using AES-256-GCM (same key as merchant_crm).
//! Performs basic OCR-style name matching against registry data.

use super::models::DocumentType;
use aes_gcm::{aead::{Aead, KeyInit}, Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::RngCore;
use sha2::{Digest, Sha256};

const NONCE_LEN: usize = 12;
const STORAGE_DIR: &str = "/tmp/kyb_documents"; // Override via KYB_DOCUMENT_PATH env var

pub struct DocumentStorageService;

impl DocumentStorageService {
    /// Encrypt and store a document. Returns (file_path, file_hash, encrypted_bytes).
    pub fn store(
        kyb_application_id: uuid::Uuid,
        document_type: &DocumentType,
        file_name: &str,
        content: &[u8],
    ) -> Result<StoredDocument, String> {
        let encrypted = encrypt(content)?;
        let hash = hex::encode(Sha256::digest(content));

        let dir = std::env::var("KYB_DOCUMENT_PATH").unwrap_or_else(|_| STORAGE_DIR.to_string());
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        let file_path = format!("{}/{}_{}_{}", dir, kyb_application_id, document_type, file_name);
        std::fs::write(&file_path, &encrypted).map_err(|e| e.to_string())?;

        Ok(StoredDocument { file_path, file_hash: hash })
    }

    /// Decrypt a stored document.
    pub fn retrieve(file_path: &str) -> Result<Vec<u8>, String> {
        let encrypted = std::fs::read(file_path).map_err(|e| e.to_string())?;
        decrypt(&encrypted)
    }

    /// Basic OCR simulation: check if the business name appears in the document text.
    /// In production, replace with a real OCR provider (e.g., Google Vision, AWS Textract).
    pub fn extract_and_validate(content: &[u8], expected_name: &str) -> OcrResult {
        // Attempt UTF-8 decode (works for text-based PDFs / plain text)
        let text = String::from_utf8_lossy(content).to_lowercase();
        let name_lower = expected_name.to_lowercase();
        let confidence = if text.contains(&name_lower) { 0.95 } else { 0.30 };

        OcrResult {
            extracted_text: text.chars().take(500).collect(),
            name_match: confidence > 0.5,
            confidence,
        }
    }
}

pub struct StoredDocument {
    pub file_path: String,
    pub file_hash: String,
}

pub struct OcrResult {
    pub extracted_text: String,
    pub name_match: bool,
    pub confidence: f64,
}

// ── AES-256-GCM helpers ───────────────────────────────────────────────────────

fn load_key() -> Result<Key<Aes256Gcm>, String> {
    let hex = std::env::var("ENCRYPTION_KEY")
        .map_err(|_| "ENCRYPTION_KEY not set".to_string())?;
    let bytes = hex::decode(&hex).map_err(|e| format!("Invalid ENCRYPTION_KEY: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("ENCRYPTION_KEY must be 32 bytes, got {}", bytes.len()));
    }
    Ok(*Key::<Aes256Gcm>::from_slice(&bytes))
}

fn encrypt(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let key = load_key()?;
    let cipher = Aes256Gcm::new(&key);
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher.encrypt(nonce, plaintext).map_err(|e| e.to_string())?;
    // Prepend nonce so we can decrypt later
    let mut out = nonce_bytes.to_vec();
    out.extend(ciphertext);
    Ok(out)
}

fn decrypt(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < NONCE_LEN {
        return Err("Data too short".to_string());
    }
    let key = load_key()?;
    let cipher = Aes256Gcm::new(&key);
    let nonce = Nonce::from_slice(&data[..NONCE_LEN]);
    cipher.decrypt(nonce, &data[NONCE_LEN..]).map_err(|e| e.to_string())
}
