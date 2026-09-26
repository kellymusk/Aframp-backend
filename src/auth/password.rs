use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2, Params, Algorithm, Version,
};

/// Argon2id parameters chosen to meet OWASP's minimum recommendations
/// (https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html):
///
///   m = 19 456 KiB  — memory cost (~19 MiB)
///   t = 2           — time cost (iterations)
///   p = 1           — parallelism
///
/// These values are pinned explicitly so that a future update to the `argon2`
/// crate's defaults cannot silently reduce the security of newly created
/// password hashes. Existing hashes carry their parameters in the PHC string
/// and will continue to verify correctly regardless of these constants.
const ARGON2_M_COST: u32 = 19_456;
const ARGON2_T_COST: u32 = 2;
const ARGON2_P_COST: u32 = 1;

fn argon2() -> Argon2<'static> {
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, None)
        .expect("ARGON2 params are statically valid — if this panics the constants are wrong");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub fn hash(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    argon2()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
}

pub fn verify(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    argon2()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `hash` produces a valid PHC string using Argon2id with
    /// parameters that meet the OWASP minimums: m ≥ 19 456, t ≥ 2, p ≥ 1.
    #[test]
    fn hash_params_meet_owasp_minimums() {
        let h = hash("hunter2").expect("hashing must succeed");
        let parsed = PasswordHash::new(&h).expect("produced a valid PHC string");

        // The algorithm must be argon2id.
        assert_eq!(parsed.algorithm.as_str(), "argon2id");

        let get = |name: &str| -> u32 {
            parsed
                .params
                .get_str(name)
                .expect("param present")
                .parse()
                .expect("param is an integer")
        };

        let m = get("m");
        let t = get("t");
        let p = get("p");

        assert!(
            m >= 19_456,
            "memory cost {m} KiB is below the OWASP minimum of 19 456 KiB"
        );
        assert!(t >= 2, "time cost {t} is below the OWASP minimum of 2");
        assert!(p >= 1, "parallelism {p} is below the OWASP minimum of 1");
    }

    /// Round-trip: a freshly hashed password must verify successfully.
    #[test]
    fn hash_and_verify_round_trip() {
        let h = hash("correct-horse-battery-staple").expect("hashing must succeed");
        assert!(verify("correct-horse-battery-staple", &h));
        assert!(!verify("wrong-password", &h));
    }
}
