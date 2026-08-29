//! End-to-end proof of the "scan and pay" loop against Stellar testnet.
//!
//! This is a **test/demo harness, not production code** — the Aframp backend
//! deliberately never signs Stellar transactions itself (customers pay from
//! their own wallets). This binary stands in for that customer wallet so the
//! full loop can be exercised without a human with a phone.
//!
//! What it does, in order:
//!   1. signs up a throwaway merchant and creates their Stellar wallet
//!   2. creates a payment request → gets a memo + SEP-0007 QR URI
//!   3. funds the merchant wallet via friendbot (a `payment` op to an account
//!      that doesn't exist on-ledger yet fails with `op_no_destination`, so the
//!      destination has to be created first)
//!   4. creates + funds a throwaway "customer" account
//!   5. builds, signs, and submits a real memo-tagged payment to the merchant
//!   6. polls the public payment-request endpoint until it flips to `paid`
//!
//! Run with the backend up:
//!   cargo run --example prove_payment_loop

use std::time::Duration;

use serde_json::Value;
use stellar_base::amount::Stroops;
use stellar_base::asset::Asset;
use stellar_base::crypto::{DalekKeyPair, PublicKey};
use stellar_base::memo::Memo;
use stellar_base::network::Network;
use stellar_base::operations::Operation;
use stellar_base::transaction::{Transaction, MIN_BASE_FEE};
use stellar_base::xdr::XDRSerialize;

const API: &str = "http://127.0.0.1:3000";
const HORIZON: &str = "https://horizon-testnet.stellar.org";
const FRIENDBOT: &str = "https://friendbot.stellar.org";
const AMOUNT_STROOPS: i64 = 25_000_000; // 2.5 XLM

type Err = Box<dyn std::error::Error>;

#[tokio::main]
async fn main() -> Result<(), Err> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    // 1. throwaway merchant ------------------------------------------------
    let email = format!("proof+{}@aframp.dev", uuid::Uuid::new_v4().simple());
    let signup: Value = http
        .post(format!("{API}/signup"))
        .json(&serde_json::json!({
            "email": email, "password": "correcthorsebattery", "name": "Loop Proof"
        }))
        .send()
        .await?
        .json()
        .await?;
    let token = signup["token"].as_str().ok_or("signup failed")?.to_string();
    println!("[1/6] merchant signed up: {}", signup["merchant_id"]);

    let wallet: Value = http
        .post(format!("{API}/wallet/create"))
        .bearer_auth(&token)
        .json(&serde_json::json!({}))
        .send()
        .await?
        .json()
        .await?;
    let merchant_address = wallet["address"].as_str().ok_or("wallet create failed")?.to_string();
    println!("[1/6] merchant wallet: {merchant_address}");

    // 2. payment request ---------------------------------------------------
    let pr: Value = http
        .post(format!("{API}/payment-requests"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "amount_stroops": AMOUNT_STROOPS }))
        .send()
        .await?
        .json()
        .await?;
    let pr_id = pr["id"].as_str().ok_or("payment request failed")?.to_string();
    let memo = pr["memo"].as_str().ok_or("no memo")?.to_string();
    println!("[2/6] payment request {pr_id}");
    println!("      memo:     {memo}");
    println!("      sep7_uri: {}", pr["sep7_uri"]);
    println!("      status:   {}", pr["status"]);

    // 3. fund the merchant wallet so it exists on-ledger --------------------
    friendbot(&http, &merchant_address).await?;
    println!("[3/6] merchant wallet funded on-ledger via friendbot");

    // 4. throwaway "customer" account --------------------------------------
    let customer = DalekKeyPair::random()?;
    let customer_address = customer.public_key().account_id();
    friendbot(&http, &customer_address).await?;
    println!("[4/6] customer account funded: {customer_address}");

    // 5. build + sign + submit a real memo-tagged payment -------------------
    let sequence = account_sequence(&http, &customer_address).await?;
    let payment = Operation::new_payment()
        .with_destination(PublicKey::from_account_id(&merchant_address)?)
        .with_amount(Stroops::new(AMOUNT_STROOPS))?
        .with_asset(Asset::new_native())
        .build()?;

    let mut tx = Transaction::builder(
        customer.public_key(),
        sequence + 1,
        MIN_BASE_FEE,
    )
    .with_memo(Memo::new_text(memo.clone())?)
    .add_operation(payment)
    .into_transaction()?;

    tx.sign(customer.as_ref(), &Network::new_test())?;
    let envelope = tx.into_envelope().xdr_base64()?;

    let submit: Value = http
        .post(format!("{HORIZON}/transactions"))
        .form(&[("tx", envelope.as_str())])
        .send()
        .await?
        .json()
        .await?;

    match submit["hash"].as_str() {
        Some(hash) => println!("[5/6] payment submitted, tx {hash}"),
        None => {
            println!("[5/6] SUBMIT FAILED:\n{}", serde_json::to_string_pretty(&submit)?);
            return Err("transaction submission failed".into());
        }
    }

    // 6. wait for the backend's poller to detect + correlate it ------------
    println!("[6/6] waiting for the deposit worker to correlate memo -> request…");
    for attempt in 1..=40 {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let current: Value = http
            .get(format!("{API}/payment-requests/{pr_id}"))
            .send()
            .await?
            .json()
            .await?;
        let status = current["status"].as_str().unwrap_or("?");
        println!("      [{:>3}s] status = {status}", attempt * 5);
        if status == "paid" {
            println!("\n✅ PROVEN: real memo-tagged testnet payment correlated to its request.");
            println!("{}", serde_json::to_string_pretty(&current)?);
            return Ok(());
        }
        if status == "expired" {
            return Err("payment request expired before it was detected".into());
        }
    }
    Err("timed out waiting for the request to flip to paid".into())
}

async fn friendbot(http: &reqwest::Client, address: &str) -> Result<(), Err> {
    let res: Value = http
        .get(format!("{FRIENDBOT}/?addr={address}"))
        .send()
        .await?
        .json()
        .await?;
    // Already-funded accounts come back as an error; that's fine for a re-run.
    if res.get("hash").is_none() && res.get("successful").is_none() {
        let detail = res["detail"].as_str().unwrap_or("");
        if !detail.contains("createAccountAlreadyExist") && !detail.is_empty() {
            println!("      (friendbot note: {detail})");
        }
    }
    Ok(())
}

async fn account_sequence(http: &reqwest::Client, address: &str) -> Result<i64, Err> {
    let account: Value = http
        .get(format!("{HORIZON}/accounts/{address}"))
        .send()
        .await?
        .json()
        .await?;
    account["sequence"]
        .as_str()
        .ok_or("no sequence on account")?
        .parse::<i64>()
        .map_err(Into::into)
}
