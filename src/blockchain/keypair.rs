use ed25519_dalek::SigningKey;
use rand::RngCore;
use stellar_strkey::ed25519::{PrivateKey, PublicKey};

pub struct StellarKeypair {
    pub public_address: String,
    pub secret_seed: String,
}

pub fn generate_keypair() -> StellarKeypair {
    let mut seed = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut seed);

    let signing_key = SigningKey::from_bytes(&seed);
    let public_bytes = signing_key.verifying_key().to_bytes();

    StellarKeypair {
        public_address: PublicKey(public_bytes).to_string().as_str().to_owned(),
        secret_seed: PrivateKey(seed).as_unredacted().to_string().as_str().to_owned(),
    }
}
