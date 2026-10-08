use sha2::{Digest, Sha256};

/// Hash of a full API key as stored in `api_keys.secret_hash`. Keys are
/// high-entropy random strings, so a plain SHA-256 is sufficient — unlike
/// passwords they can't be brute-forced from the digest.
pub fn hash(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}
