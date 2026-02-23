#[cfg(test)]
mod webhook_tests {
    use serde_json::json;

    #[test]
    fn test_webhook_error_display() {
        use Bitmesh_backend::services::webhook_processor::WebhookProcessorError;

        let err = WebhookProcessorError::InvalidSignature;
        assert_eq!(err.to_string(), "Invalid signature");

        let err = WebhookProcessorError::AlreadyProcessed;
        assert_eq!(err.to_string(), "Already processed");

        let err = WebhookProcessorError::UnknownProvider("test".to_string());
        assert_eq!(err.to_string(), "Unknown provider: test");
    }

    #[test]
    fn test_event_id_extraction_flutterwave() {
        let payload = json!({
            "id": 12345,
            "event": "charge.completed",
            "data": {
                "tx_ref": "tx_123",
                "status": "successful"
            }
        });

        let event_id = payload
            .get("id")
            .and_then(|v| v.as_i64())
            .map(|id| id.to_string());
        assert_eq!(event_id, Some("12345".to_string()));
    }

    #[test]
    fn test_event_id_extraction_paystack() {
        let payload = json!({
            "id": 67890,
            "event": "charge.success",
            "data": {
                "reference": "tx_456",
                "status": "success"
            }
        });

        let event_id = payload
            .get("id")
            .and_then(|v| v.as_i64())
            .map(|id| id.to_string());
        assert_eq!(event_id, Some("67890".to_string()));
    }

    #[test]
    fn test_event_type_mapping() {
        let events = vec![
            ("charge.completed", "payment_success"),
            ("charge.success", "payment_success"),
            ("charge.failed", "payment_failure"),
            ("transfer.completed", "withdrawal_success"),
            ("transfer.success", "withdrawal_success"),
            ("transfer.failed", "withdrawal_failure"),
        ];

        for (event_type, expected_action) in events {
            let action = match event_type {
                "charge.completed" | "charge.success" => "payment_success",
                "charge.failed" => "payment_failure",
                "transfer.completed" | "transfer.success" => "withdrawal_success",
                "transfer.failed" => "withdrawal_failure",
                _ => "unknown",
            };
            assert_eq!(action, expected_action, "Failed for event: {}", event_type);
        }
    }

    #[test]
    fn test_webhook_payload_parsing() {
        let flutterwave_payload = json!({
            "id": 123,
            "event": "charge.completed",
            "data": {
                "tx_ref": "tx_123",
                "status": "successful",
                "amount": 5000,
                "currency": "NGN"
            }
        });

        assert!(flutterwave_payload.get("event").is_some());
        assert!(flutterwave_payload.get("data").is_some());

        let data = flutterwave_payload.get("data").unwrap();
        assert_eq!(data.get("tx_ref").and_then(|v| v.as_str()), Some("tx_123"));
        assert_eq!(
            data.get("status").and_then(|v| v.as_str()),
            Some("successful")
        );

        let paystack_payload = json!({
            "event": "charge.success",
            "data": {
                "reference": "tx_456",
                "status": "success",
                "amount": 500000
            }
        });

        assert!(paystack_payload.get("event").is_some());
        let data = paystack_payload.get("data").unwrap();
        assert_eq!(
            data.get("reference").and_then(|v| v.as_str()),
            Some("tx_456")
        );
    }
}
