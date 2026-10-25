//! Unit tests for `aframp::blockchain::wallet_crypto`.
//!
//! Covers the encrypt/decrypt round-trip, nonce randomness, wrong-key rejection,
//! truncated-ciphertext rejection, and `parse_key` validation.
//!
//! These tests do **not** require a database — run them with:
//!
//! ```bash
//! cargo test --test wallet_crypto_tests
//! ```

use aframp::blockchain::wallet_crypto::{decrypt, encrypt, parse_key};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A valid 32-byte key expressed as 64 hex characters.
const VALID_HEX_KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn test_key() -> [u8; 32] {
    parse_key(VALID_HEX_KEY).expect("test key must be valid")
}

// ---------------------------------------------------------------------------
// parse_key
// ---------------------------------------------------------------------------

#[test]
fn parse_key_accepts_32_byte_hex() {
    let result = parse_key(VALID_HEX_KEY);
    assert!(result.is_ok(), "expected Ok, got {result:?}");
    assert_eq!(result.unwrap().len(), 32);
}

#[test]
fn parse_key_rejects_too_short_hex() {
    // 31 bytes = 62 hex chars — one byte short of the required 32.
    let short = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e";
    assert!(
        parse_key(short).is_err(),
        "expected Err for 31-byte key, got Ok"
    );
}

#[test]
fn parse_key_rejects_too_long_hex() {
    // 33 bytes = 66 hex chars — one byte over the required 32.
    let long = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
    assert!(
        parse_key(long).is_err(),
        "expected Err for 33-byte key, got Ok"
    );
}

#[test]
fn parse_key_rejects_non_hex_input() {
    assert!(
        parse_key("not-valid-hex-at-all-zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_err(),
        "expected Err for non-hex input"
    );
}

#[test]
fn parse_key_rejects_empty_string() {
    assert!(
        parse_key("").is_err(),
        "expected Err for empty key string"
    );
}

// ---------------------------------------------------------------------------
// encrypt / decrypt round-trip
// ---------------------------------------------------------------------------

#[test]
fn encrypt_then_decrypt_returns_original_plaintext() {
    let key = test_key();
    let plaintext = "SCZANGKATMZM7A7IWGKZQZ3ZZHBK7BSL4G3MXXX"; // fake Stellar secret seed

    let ciphertext = encrypt(&key, plaintext).expect("encrypt must succeed");
    let recovered = decrypt(&key, &ciphertext).expect("decrypt must succeed");

    assert_eq!(
        recovered, plaintext,
        "decrypted value must equal original plaintext"
    );
}

#[test]
fn encrypt_empty_string_round_trips() {
    let key = test_key();
    let ciphertext = encrypt(&key, "").expect("encrypt of empty string must succeed");
    let recovered = decrypt(&key, &ciphertext).expect("decrypt must succeed");
    assert_eq!(recovered, "", "empty string must round-trip through encrypt/decrypt");
}

#[test]
fn encrypt_unicode_plaintext_round_trips() {
    let key = test_key();
    let plaintext = "Héllo Wörld — Àframp 🌍";
    let ciphertext = encrypt(&key, plaintext).expect("encrypt must succeed");
    let recovered = decrypt(&key, &ciphertext).expect("decrypt must succeed");
    assert_eq!(recovered, plaintext);
}

// ---------------------------------------------------------------------------
// Nonce randomness: two encryptions of the same plaintext differ
// ---------------------------------------------------------------------------

#[test]
fn two_encryptions_of_same_plaintext_produce_different_ciphertexts() {
    let key = test_key();
    let plaintext = "SCZANGKATMZM7A7IWGKZQZ3ZZHBK7BSL4G3MXXX";

    let ct1 = encrypt(&key, plaintext).expect("first encrypt must succeed");
    let ct2 = encrypt(&key, plaintext).expect("second encrypt must succeed");

    assert_ne!(
        ct1, ct2,
        "two encryptions of the same plaintext must produce different ciphertexts (random nonce)"
    );
}

// ---------------------------------------------------------------------------
// Wrong-key rejection
// ---------------------------------------------------------------------------

#[test]
fn decrypt_with_wrong_key_returns_err() {
    let key = test_key();
    let plaintext = "SCZANGKATMZM7A7IWGKZQZ3ZZHBK7BSL4G3MXXX";

    let ciphertext = encrypt(&key, plaintext).expect("encrypt must succeed");

    // Different key — every byte flipped.
    let wrong_key_hex = "fffefdfcfbfaf9f8f7f6f5f4f3f2f1f0efeeedecebeae9e8e7e6e5e4e3e2e1e0";
    let wrong_key = parse_key(wrong_key_hex).expect("wrong test key must be valid");

    let result = decrypt(&wrong_key, &ciphertext);
    assert!(
        result.is_err(),
        "decrypt with a wrong key must return Err, got Ok({:?})",
        result.ok()
    );
}

// ---------------------------------------------------------------------------
// Truncated ciphertext rejection
// ---------------------------------------------------------------------------

#[test]
fn decrypt_of_empty_string_returns_err() {
    let key = test_key();
    // An empty hex string decodes to zero bytes — shorter than the 12-byte nonce.
    assert!(
        decrypt(&key, "").is_err(),
        "decrypt of empty string must return Err"
    );
}

#[test]
fn decrypt_of_nonce_only_returns_err() {
    let key = test_key();
    // 12 zero bytes = 24 hex chars; nonce is present but there's no ciphertext or tag.
    let nonce_only = "000000000000000000000000";
    assert!(
        decrypt(&key, nonce_only).is_err(),
        "decrypt of nonce-only ciphertext must return Err"
    );
}

#[test]
fn decrypt_of_truncated_ciphertext_returns_err() {
    let key = test_key();
    let plaintext = "SCZANGKATMZM7A7IWGKZQZ3ZZHBK7BSL4G3MXXX";

    let ciphertext_hex = encrypt(&key, plaintext).expect("encrypt must succeed");

    // Truncate to 30 hex characters (15 bytes) — keeps part of the nonce but
    // removes all of the actual ciphertext and authentication tag.
    let truncated = &ciphertext_hex[..30];

    assert!(
        decrypt(&key, truncated).is_err(),
        "decrypt of truncated ciphertext must return Err"
    );
}

#[test]
fn decrypt_of_non_hex_returns_err() {
    let key = test_key();
    assert!(
        decrypt(&key, "not-hex-at-all!!").is_err(),
        "decrypt of non-hex input must return Err"
    );
}
