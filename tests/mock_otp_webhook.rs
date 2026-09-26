//! #1132 — MockOtpProvider webhook signature contract.
//!
//! The mock accepts a configurable expected signature for tests: the sentinel
//! `mock-signature` (see `scripts/mock_otp.ts`). This test locks that contract
//! without editing protected `src/otp/mock.rs`.

fn mock_signature_from_script() -> String {
    let src = std::fs::read_to_string("scripts/mock_otp.ts").expect("scripts/mock_otp.ts");
    for line in src.lines() {
        let line = line.trim();
        if line.starts_with("export const MOCK_OTP_WEBHOOK_SIGNATURE") {
            let start = line.find('"').expect("signature string");
            let end = line.rfind('"').expect("signature string end");
            return line[start + 1..end].to_string();
        }
    }
    panic!("MOCK_OTP_WEBHOOK_SIGNATURE not found");
}

#[test]
fn mock_otp_provider_uses_documented_webhook_sentinel() {
    let expected = mock_signature_from_script();
    assert_eq!(expected, "mock-signature");

    let mock_src = std::fs::read_to_string("src/otp/mock.rs").expect("src/otp/mock.rs");
    assert!(
        mock_src.contains(&format!("\"{expected}\"")),
        "MockOtpProvider must accept the documented sentinel {expected:?}"
    );
    assert!(
        mock_src.contains("verify_webhook_signature"),
        "mock must implement verify_webhook_signature"
    );
}
