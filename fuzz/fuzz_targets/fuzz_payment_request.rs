#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Try to parse as JSON
    if let Ok(json_str) = std::str::from_utf8(data) {
        let _: Result<serde_json::Value, _> = serde_json::from_str(json_str);
        
        // Try to deserialize as CreatePaymentRequestRequest specifically
        #[derive(serde::Deserialize)]
        struct CreatePaymentRequestRequest {
            amount_stroops: i64,
            asset: Option<String>,
            expires_in_secs: Option<i64>,
        }
        
        let _: Result<CreatePaymentRequestRequest, _> = serde_json::from_str(json_str);
    }
});
