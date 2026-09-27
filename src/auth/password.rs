//! Password hashing and verification.
//!
//! Uses Argon2id with explicitly configured parameters rather than
//! `Argon2::default()`. Relying on the crate defaults is risky: the defaults
//! can silently change between argon2 crate versions, which would make newly
//! hashed passwords weaker (or stronger) without any code change on our side.
//!
//! Chosen parameters follow the OWASP Password Storage Cheat Sheet
//! recommendation for Argon2id (as of the 2024 guidance):
//!   - memory cost (m) = 19456 KiB (~19 MiB)
//!   - time cost   (t) = 2 iterations
//!   - parallelism (p) = 1 lane
//!
//! These values are a deliberate balance between resistance to offline
//! cracking and acceptable latency/memory on the server. Existing hashes
//! created with the previous defaults still verify correctly because the
//! parameters are encoded in the PHC hash string itself.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Algorithm, Argon2, Params, Version,
};

/// OWASP-recommended Argon2id memory cost in KiB (~19 MiB).
pub const ARGON2_MEMORY_COST_KIB: u32 = 19456;
/// OWASP-recommended Argon2id time cost (iterations).
pub const ARGON2_TIME_COST: u32 = 2;
/// OWASP-recommended Argon2id parallelism (lanes).
pub const ARGON2_PARALLELISM: u32 = 1;

/// Build the explicitly configured Argon2id instance used for hashing and
/// verification. See the module-level docs for the rationale behind the
/// parameter choices.
fn argon2() -> Argon2<'static> {
    let params = Params::new(
        ARGON2_MEMORY_COST_KIB,
        ARGON2_TIME_COST,
        ARGON2_PARALLELISM,
        None,
    )
    .expect("valid Argon2 parameters");

    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Hash a plaintext password using Argon2id with the configured parameters.
pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon2().hash_password(password.as_bytes(), &salt)?;
    Ok(hash.to_string())
}

/// Verify a plaintext password against a stored PHC hash string.
pub fn verify_password(password: &str, hash: &str) -> Result<bool, argon2::password_hash::Error> {
    let parsed = PasswordHash::new(hash)?;
    Ok(argon2()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_uses_configured_argon2id_params() {
        let hash = hash_password("correct horse battery staple").expect("hashing succeeds");
        let parsed = PasswordHash::new(&hash).expect("valid PHC string");

        assert_eq!(parsed.algorithm.as_str(), "argon2id");

        let params = Params::try_from(&parsed).expect("params parse from hash");
        assert_eq!(params.m_cost(), ARGON2_MEMORY_COST_KIB);
        assert_eq!(params.t_cost(), ARGON2_TIME_COST);
        assert_eq!(params.p_cost(), ARGON2_PARALLELISM);

        // Guard against regressions below the OWASP minimums.
        assert!(params.m_cost() >= 19456, "memory cost below OWASP minimum");
        assert!(params.t_cost() >= 2, "time cost below OWASP minimum");
        assert!(params.p_cost() >= 1, "parallelism below OWASP minimum");
    }

    #[test]
    fn verify_roundtrip() {
        let hash = hash_password("s3cret-password").expect("hashing succeeds");
        assert!(verify_password("s3cret-password", &hash).expect("verification succeeds"));
        assert!(!verify_password("wrong-password", &hash).expect("verification succeeds"));
    }
}
