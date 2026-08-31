//! `--rotate-key` CLI subcommand: re-encrypts every wallet's
//! `secret_key_encrypted` column from `WALLET_ENCRYPTION_KEY` (the current
//! key) to `WALLET_ENCRYPTION_KEY_NEW` (the key you're rotating to).
//!
//! # Why this is safe to run without downtime today
//!
//! No request-serving code path in this codebase calls
//! [`crate::blockchain::wallet_crypto::decrypt`] — wallet secrets are
//! encrypted at creation time and never read back by anything the running
//! server does today (withdrawals settle through Paystack, not by signing
//! with the wallet's own Stellar key). That means there is no live reader to
//! race against: this tool can walk the table and re-encrypt every row while
//! the server keeps running on the *old* key, because the old key isn't
//! being used for anything concurrent with the rotation.
//!
//! # Operational sequence
//!
//! 1. Generate a new key: `openssl rand -hex 32`.
//! 2. Set `WALLET_ENCRYPTION_KEY_NEW` to that value alongside the existing
//!    `WALLET_ENCRYPTION_KEY` and `DATABASE_URL` (pointed at the target
//!    database), then run the binary with `--rotate-key`.
//! 3. On success, update the deployment's `WALLET_ENCRYPTION_KEY` to the new
//!    key's value and restart the server. Discard the old key.
//!
//! # If this stops being true
//!
//! The moment a live code path starts calling `decrypt` (e.g. a sweep-wallet
//! signer), this approach needs revisiting: either pause writers/readers of
//! `secret_key_encrypted` for the rotation's duration, or version-tag the
//! ciphertext (a key-id prefix) so `decrypt` can try the outgoing key during
//! a transition window instead of assuming exactly one key is ever current.

use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use crate::blockchain::wallet_crypto;

/// Runs the rotation. Returns `Err` (and leaves every already-rotated row on
/// the new key) if any row fails to decrypt with the old key or the two keys
/// are identical — a partial rotation is safe to re-run, since only rows
/// still holding old-key ciphertext will fail to re-encrypt a second time.
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = require_env("DATABASE_URL")?;
    let old_key = wallet_crypto::parse_key(&require_env("WALLET_ENCRYPTION_KEY")?)?;
    let new_key = wallet_crypto::parse_key(&require_env("WALLET_ENCRYPTION_KEY_NEW")?)?;
    if old_key == new_key {
        return Err("WALLET_ENCRYPTION_KEY_NEW must differ from WALLET_ENCRYPTION_KEY".into());
    }

    let db = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, secret_key_encrypted FROM wallets")
            .fetch_all(&db)
            .await?;

    let total = rows.len();
    tracing::info!(total, "starting WALLET_ENCRYPTION_KEY rotation");

    let mut rotated = 0usize;
    let mut failed = 0usize;

    for (id, encrypted) in rows {
        let plaintext = match wallet_crypto::decrypt(&old_key, &encrypted) {
            Ok(p) => p,
            Err(err) => {
                failed += 1;
                tracing::error!(wallet_id = %id, error = %err, "failed to decrypt with WALLET_ENCRYPTION_KEY; row left untouched");
                continue;
            }
        };
        let re_encrypted = wallet_crypto::encrypt(&new_key, &plaintext)?;

        // A single UPDATE is already atomic per row: either this wallet's
        // ciphertext moves to the new key or it doesn't, never a partial
        // write. There's no multi-statement unit of work here that needs an
        // explicit transaction.
        sqlx::query("UPDATE wallets SET secret_key_encrypted = $2 WHERE id = $1")
            .bind(id)
            .bind(&re_encrypted)
            .execute(&db)
            .await?;
        rotated += 1;
    }

    tracing::info!(rotated, failed, total, "WALLET_ENCRYPTION_KEY rotation finished");

    if failed > 0 {
        return Err(format!(
            "{failed} of {total} wallet row(s) could not be decrypted with WALLET_ENCRYPTION_KEY \
             and were left untouched — see logs above for wallet ids. Do not switch the deployment \
             to WALLET_ENCRYPTION_KEY_NEW yet; investigate those rows and re-run this command."
        )
        .into());
    }

    println!(
        "Rotation complete: {rotated} wallet(s) re-encrypted with WALLET_ENCRYPTION_KEY_NEW.\n\
         Next: set the deployment's WALLET_ENCRYPTION_KEY to the value you used for \
         WALLET_ENCRYPTION_KEY_NEW, restart the server, and discard the old key."
    );
    Ok(())
}

fn require_env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is required for --rotate-key"))
}
