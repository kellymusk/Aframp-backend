use ed25519_dalek::SigningKey;
use rand::RngCore;
use stellar_strkey::ed25519::{PrivateKey, PublicKey};

pub struct StellarKeypair {
    pub public_address: String,
    pub secret_seed: String,
}

#[derive(Debug, thiserror::Error)]
pub enum KeypairError {
    #[error("OsRng failed while generating Stellar keypair: {0}")]
    OsRng(String),
}

/// Generate a Stellar keypair. Returns `Err` if the OS CSPRNG is unavailable
/// (e.g. `/dev/urandom` missing in a sandbox) instead of panicking the Tokio task.
pub fn generate_keypair() -> Result<StellarKeypair, KeypairError> {
    let mut seed = [0u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut seed)
        .map_err(|e| KeypairError::OsRng(e.to_string()))?;

    let signing_key = SigningKey::from_bytes(&seed);
    let public_bytes = signing_key.verifying_key().to_bytes();

    Ok(StellarKeypair {
        public_address: PublicKey(public_bytes).to_string().as_str().to_owned(),
        secret_seed: PrivateKey(seed).as_unredacted().to_string().as_str().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_keypair_returns_result() {
        let result: Result<StellarKeypair, KeypairError> = generate_keypair();
        let kp = result.expect("OsRng should succeed in a normal test environment");
        assert!(kp.public_address.starts_with('G'));
        assert!(kp.secret_seed.starts_with('S'));
        assert_ne!(kp.public_address, kp.secret_seed);
    }
}
