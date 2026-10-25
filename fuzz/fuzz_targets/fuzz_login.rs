#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Try to parse as JSON
    if let Ok(json_str) = std::str::from_utf8(data) {
        let _: Result<serde_json::Value, _> = serde_json::from_str(json_str);
        
        // Try to deserialize as LoginRequest specifically
        #[derive(serde::Deserialize)]
        struct LoginRequest {
            email: String,
            password: String,
        }
        
        let _: Result<LoginRequest, _> = serde_json::from_str(json_str);
    }
});
