/**
 * #1132 — MockOtpProvider webhook signature sentinel for tests.
 *
 * When `OTP_PROVIDER=mock`, `MockOtpProvider::verify_webhook_signature`
 * returns true only if the `X-Termii-Signature` header equals this value.
 * Integration tests configure the "expected signature" by sending this
 * string (see `tests/webhook_flow.rs` and CONTRIBUTING.md).
 */
export const MOCK_OTP_WEBHOOK_SIGNATURE = "mock-signature" as const;
