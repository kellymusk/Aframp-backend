use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;

const NONCE_LEN: usize = 12;

pub fn parse_key(hex_key: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex_key).map_err(|e| format!("WALLET_ENCRYPTION_KEY is not valid hex: {e}"))?;
    bytes
        .try_into()
        .map_err(|_| "WALLET_ENCRYPTION_KEY must decode to exactly 32 bytes".to_string())
}

pub fn encrypt(key: &[u8; 32], plaintext: &str) -> Result<String, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(hex::encode(out))
}

pub fn decrypt(key: &[u8; 32], encoded: &str) -> Result<String, String> {
    let bytes = hex::decode(encoded).map_err(|e| e.to_string())?;
    if bytes.len() < NONCE_LEN {
        return Err("ciphertext too short".into());
    }
    let (nonce_bytes, ciphertext) = bytes.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher.decrypt(nonce, ciphertext).map_err(|e| e.to_string())?;
    String::from_utf8(plaintext).map_err(|e| e.to_string())
}
