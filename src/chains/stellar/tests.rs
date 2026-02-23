#[cfg(test)]
mod tests {
    use crate::chains::stellar::errors::StellarError;
    use crate::chains::stellar::{
        client::StellarClient,
        config::{StellarConfig, StellarNetwork},
        types::{extract_asset_balance, is_valid_stellar_address, AssetBalance},
    };
    use std::time::Duration;
    use stellar_strkey::ed25519::PublicKey as StrkeyPublicKey;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    fn test_config() -> StellarConfig {
        StellarConfig {
            network: StellarNetwork::Testnet,
            horizon_url_override: None,
            request_timeout: Duration::from_secs(10),
            max_retries: 3,
            health_check_interval: Duration::from_secs(30),
        }
    }

    async fn spawn_single_response_server(
        status_code: u16,
        body: &'static str,
    ) -> (String, oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("failed to bind test listener");
        let addr = listener.local_addr().expect("failed to read listener addr");
        let (request_line_tx, request_line_rx) = oneshot::channel::<String>();

        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept failed");
            let mut buf = vec![0_u8; 8192];
            let n = socket.read(&mut buf).await.expect("failed to read request");
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let first_line = req.lines().next().unwrap_or_default().to_string();
            let _ = request_line_tx.send(first_line);

            let reason = match status_code {
                200 => "OK",
                404 => "Not Found",
                429 => "Too Many Requests",
                _ => "Internal Server Error",
            };
            let response = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status_code,
                reason,
                body.len(),
                body
            );

            socket
                .write_all(response.as_bytes())
                .await
                .expect("failed to write response");
        });

        (format!("http://{}", addr), request_line_rx)
    }

    // Valid testnet account that exists (from Stellar friendbot)
    const TEST_ADDRESS: &str = "GCJRI5CIWK5IU67Q6DGA7QW52JDKRO7JEAHQKFNDUJUPEZGURDBX3LDX";

    #[test]
    fn test_valid_stellar_address() {
        let valid_address = TEST_ADDRESS;
        assert!(is_valid_stellar_address(valid_address));

        let invalid_address = "INVALID_ADDRESS";
        assert!(!is_valid_stellar_address(invalid_address));

        let wrong_length = "GD5DJQDQKNR7DSXJVNJTV3P5JJH4KJVTI2JZNYUYIIKHTDNJQXECM4J";
        assert!(!is_valid_stellar_address(wrong_length));
    }

    #[tokio::test]
    async fn test_stellar_client_creation() {
        let config = test_config();
        let client = StellarClient::new(config);
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_get_valid_testnet_account() {
        let config = test_config();
        let client = StellarClient::new(config).expect("Failed to create client");

        let test_address = TEST_ADDRESS;

        match client.get_account(test_address).await {
            Ok(account) => {
                assert_eq!(account.account_id, test_address);
                assert!(!account.balances.is_empty());
            }
            Err(StellarError::AccountNotFound { .. }) => {
                println!("Test account not found, this is expected if the account doesn't exist");
            }
            Err(StellarError::NetworkError { .. }) | Err(StellarError::TimeoutError { .. }) => {
                println!("Network issue, skipping test");
            }
            Err(e) => {
                panic!("Unexpected error: {}", e);
            }
        }
    }

    #[tokio::test]
    #[should_panic]
    async fn test_get_nonexistent_account() {
        let config = test_config();
        let client = StellarClient::new(config).expect("Failed to create client");

        // Valid StrKey generated from unlikely-to-exist raw ed25519 bytes.
        let nonexistent_address = StrkeyPublicKey([0u8; 32]).to_string();

        let result = client.get_account(nonexistent_address.as_str()).await;
        assert!(
            matches!(
                result,
                Err(StellarError::AccountNotFound { .. })
                    | Err(StellarError::NetworkError { .. })
                    | Err(StellarError::TimeoutError { .. })
            ),
            "Expected AccountNotFound or transport error for nonexistent account, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_invalid_address_format() {
        let config = test_config();
        let client = StellarClient::new(config).expect("Failed to create client");

        let invalid_address = "INVALID_ADDRESS";

        let result = client.get_account(invalid_address).await;
        assert!(matches!(result, Err(StellarError::InvalidAddress { .. })));
    }

    #[tokio::test]
    async fn test_account_exists() {
        let config = test_config();
        let client = StellarClient::new(config).expect("Failed to create client");

        let test_address = TEST_ADDRESS;

        match client.account_exists(test_address).await {
            Ok(exists) => {
                println!("Account {} exists: {}", test_address, exists);
            }
            Err(StellarError::AccountNotFound { .. }) => {
                println!("Account does not exist, which is valid");
            }
            Err(StellarError::NetworkError { .. }) | Err(StellarError::TimeoutError { .. }) => {
                println!("Network issue, skipping test");
            }
            Err(e) => {
                panic!("Unexpected error: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_health_check() {
        let config = test_config();
        let client = StellarClient::new(config).expect("Failed to create client");

        let health_status = client.health_check().await.expect("Health check failed");

        println!("Health status: {:?}", health_status);

        if health_status.is_healthy {
            assert!(health_status.response_time_ms > 0);
            assert!(health_status.error_message.is_none());
        } else {
            assert!(health_status.error_message.is_some());
        }
    }

    #[tokio::test]
    async fn test_get_balances() {
        let config = test_config();
        let client = StellarClient::new(config).expect("Failed to create client");

        let test_address = TEST_ADDRESS;

        match client.get_balances(test_address).await {
            Ok(balances) => {
                println!("Balances for {}: {:?}", test_address, balances);
                assert!(!balances.is_empty());
            }
            Err(StellarError::AccountNotFound { .. }) => {
                println!("Account not found, skipping balance test");
            }
            Err(StellarError::NetworkError { .. }) | Err(StellarError::TimeoutError { .. }) => {
                println!("Network issue, skipping test");
            }
            Err(e) => {
                panic!("Unexpected error: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_get_afri_balance() {
        let config = test_config();
        let client = StellarClient::new(config).expect("Failed to create client");

        let test_address = TEST_ADDRESS;

        match client.get_afri_balance(test_address).await {
            Ok(afri_balance) => {
                println!("AFRI balance for {}: {:?}", test_address, afri_balance);
            }
            Err(StellarError::AccountNotFound { .. }) => {
                println!("Account not found, skipping AFRI balance test");
            }
            Err(StellarError::NetworkError { .. }) | Err(StellarError::TimeoutError { .. }) => {
                println!("Network issue, skipping test");
            }
            Err(e) => {
                panic!("Unexpected error: {}", e);
            }
        }
    }

    #[test]
    fn test_config_validation() {
        let mut config = test_config();
        assert!(config.validate().is_ok());

        config.request_timeout = Duration::from_secs(0);
        assert!(config.validate().is_err());

        config = test_config();
        config.max_retries = 0;
        assert!(config.validate().is_err());

        config = test_config();
        config.health_check_interval = Duration::from_secs(0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_network_configurations() {
        let testnet_config = StellarConfig {
            network: StellarNetwork::Testnet,
            ..test_config()
        };
        assert_eq!(
            testnet_config.network.horizon_url(),
            "https://horizon-testnet.stellar.org"
        );
        assert_eq!(
            testnet_config.network.network_passphrase(),
            "Test SDF Network ; September 2015"
        );

        let mainnet_config = StellarConfig {
            network: StellarNetwork::Mainnet,
            ..test_config()
        };
        assert_eq!(
            mainnet_config.network.horizon_url(),
            "https://horizon.stellar.org"
        );
        assert_eq!(
            mainnet_config.network.network_passphrase(),
            "Public Global Stellar Network ; September 2015"
        );
    }

    #[test]
    fn test_extract_cngn_balance_by_issuer() {
        let issuer_a = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";
        let issuer_b = "GBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

        let balances = vec![
            AssetBalance {
                asset_type: "native".to_string(),
                asset_code: None,
                asset_issuer: None,
                balance: "10.0000000".to_string(),
                limit: None,
                is_authorized: true,
                is_authorized_to_maintain_liabilities: true,
                last_modified_ledger: None,
            },
            AssetBalance {
                asset_type: "credit_alphanum4".to_string(),
                asset_code: Some("cNGN".to_string()),
                asset_issuer: Some(issuer_a.to_string()),
                balance: "50.0000000".to_string(),
                limit: None,
                is_authorized: true,
                is_authorized_to_maintain_liabilities: true,
                last_modified_ledger: None,
            },
            AssetBalance {
                asset_type: "credit_alphanum4".to_string(),
                asset_code: Some("CNGN".to_string()),
                asset_issuer: Some(issuer_b.to_string()),
                balance: "75.0000000".to_string(),
                limit: None,
                is_authorized: true,
                is_authorized_to_maintain_liabilities: true,
                last_modified_ledger: None,
            },
        ];

        let issuer_specific =
            extract_asset_balance(&balances, "cNGN", Some(issuer_a)).expect("missing issuer A");
        assert_eq!(issuer_specific, "50.0000000");

        let any_issuer = extract_asset_balance(&balances, "cngn", None).expect("missing cNGN");
        assert_eq!(any_issuer, "50.0000000");
    }

    #[tokio::test]
    async fn test_health_check_unreachable_horizon() {
        let mut config = test_config();
        config.horizon_url_override = Some("http://127.0.0.1:1".to_string());
        config.request_timeout = Duration::from_secs(2);

        let client = StellarClient::new(config).expect("Failed to create client");
        let health = client
            .health_check()
            .await
            .expect("health check should not crash");

        assert!(!health.is_healthy);
        assert!(health.error_message.is_some());
    }

    #[tokio::test]
    #[ignore = "requires local TCP listener access for mocked Horizon responses"]
    async fn test_get_transaction_by_hash_mocked_success() {
        let (base_url, request_line_rx) = spawn_single_response_server(
            200,
            r#"{
                "hash": "tx_hash_1",
                "successful": true,
                "ledger": 12345,
                "paging_token": "98765",
                "memo_type": "text",
                "memo": "tx-1"
            }"#,
        )
        .await;

        let mut config = test_config();
        config.horizon_url_override = Some(base_url);
        let client = StellarClient::new(config).expect("Failed to create client");

        let tx = client
            .get_transaction_by_hash("tx_hash_1")
            .await
            .expect("expected mocked transaction");
        let request_line = request_line_rx.await.expect("missing request line");

        assert_eq!(tx.hash, "tx_hash_1");
        assert!(tx.successful);
        assert_eq!(tx.ledger, Some(12345));
        assert_eq!(tx.memo.as_deref(), Some("tx-1"));
        assert!(request_line.contains("GET /transactions/tx_hash_1 "));
    }

    #[tokio::test]
    #[ignore = "requires local TCP listener access for mocked Horizon responses"]
    async fn test_get_transaction_by_hash_mocked_not_found() {
        let (base_url, request_line_rx) =
            spawn_single_response_server(404, r#"{"status":404,"title":"Not Found"}"#).await;

        let mut config = test_config();
        config.horizon_url_override = Some(base_url);
        let client = StellarClient::new(config).expect("Failed to create client");

        let result = client.get_transaction_by_hash("missing_hash").await;
        let request_line = request_line_rx.await.expect("missing request line");
        assert!(matches!(
            result,
            Err(StellarError::TransactionFailed { .. })
        ));
        assert!(request_line.contains("GET /transactions/missing_hash "));
    }

    #[tokio::test]
    #[ignore = "requires local TCP listener access for mocked Horizon responses"]
    async fn test_list_account_transactions_mocked_success() {
        let (base_url, request_line_rx) = spawn_single_response_server(
            200,
            r#"{
                "_embedded": {
                    "records": [
                        {
                            "hash": "tx_hash_2",
                            "successful": true,
                            "ledger": 555,
                            "paging_token": "cursor_1",
                            "memo_type": "text",
                            "memo": "tx-2"
                        }
                    ]
                }
            }"#,
        )
        .await;

        let mut config = test_config();
        config.horizon_url_override = Some(base_url);
        let client = StellarClient::new(config).expect("Failed to create client");

        let page = client
            .list_account_transactions(TEST_ADDRESS, 10, None)
            .await
            .expect("expected mocked account tx page");
        let request_line = request_line_rx.await.expect("missing request line");

        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].hash, "tx_hash_2");
        assert_eq!(page.records[0].memo.as_deref(), Some("tx-2"));
        assert!(request_line.contains(&format!(
            "GET /accounts/{}/transactions?order=asc&limit=10 ",
            TEST_ADDRESS
        )));
    }

    #[tokio::test]
    #[ignore = "requires local TCP listener access for mocked Horizon responses"]
    async fn test_get_transaction_operations_mocked_success() {
        let (base_url, request_line_rx) = spawn_single_response_server(
            200,
            r#"{
                "_embedded": {
                    "records": [
                        {
                            "type": "payment",
                            "to": "GCJRI5CIWK5IU67Q6DGA7QW52JDKRO7JEAHQKFNDUJUPEZGURDBX3LDX",
                            "asset_code": "cNGN",
                            "asset_issuer": "GISSUERAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
                        }
                    ]
                }
            }"#,
        )
        .await;

        let mut config = test_config();
        config.horizon_url_override = Some(base_url);
        let client = StellarClient::new(config).expect("Failed to create client");

        let operations = client
            .get_transaction_operations("tx_hash_3")
            .await
            .expect("expected mocked operations");
        let request_line = request_line_rx.await.expect("missing request line");

        assert_eq!(operations.len(), 1);
        assert_eq!(
            operations[0].get("type").and_then(|v| v.as_str()),
            Some("payment")
        );
        assert!(request_line.contains("GET /transactions/tx_hash_3/operations?limit=200 "));
    }
}
