//! Re-encrypt every wallet secret under a new WALLET_ENCRYPTION_KEY.
//!
//! Reads DATABASE_URL, the current key from WALLET_ENCRYPTION_KEY and the
//! replacement from NEW_WALLET_ENCRYPTION_KEY. All wallets are rotated in one
//! transaction, so a failure leaves every secret under the old key. See the
//! README "Rotating WALLET_ENCRYPTION_KEY" section for the full procedure.

use aframp::blockchain::wallet_crypto;
use aframp::services::wallets;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let env = |name: &str| std::env::var(name).map_err(|_| format!("{name} is required"));

    let old_key = wallet_crypto::parse_key(&env("WALLET_ENCRYPTION_KEY")?)?;
    let new_key = wallet_crypto::parse_key(&env("NEW_WALLET_ENCRYPTION_KEY")?)
        .map_err(|e| e.replace("WALLET_ENCRYPTION_KEY", "NEW_WALLET_ENCRYPTION_KEY"))?;
    if old_key == new_key {
        return Err("NEW_WALLET_ENCRYPTION_KEY must differ from WALLET_ENCRYPTION_KEY".into());
    }

    let db = PgPoolOptions::new()
        .max_connections(1)
        .connect(&env("DATABASE_URL")?)
        .await?;
    let rotated = wallets::rotate_encryption_key(&db, &old_key, &new_key).await?;

    println!(
        "Re-encrypted {rotated} wallet secret(s). Set WALLET_ENCRYPTION_KEY to the new key and restart the API."
    );
    Ok(())
}
