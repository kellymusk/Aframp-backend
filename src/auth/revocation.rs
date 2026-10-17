//! Short-lived in-memory JWT revocation list (blocklist).
//!
//! `POST /logout` records the presented token's `jti` here together with the
//! token's own `exp`, and [`crate::auth::extractor::authenticate`] refuses any
//! token whose `jti` is still listed. Because an entry can never outlive the
//! token it describes, the set is pruned on every access, so expired entries
//! fall out automatically without a background task — the list is always
//! short-lived by construction.
//!
//! Process-local by design (same trade-off as the login rate limiter): a
//! multi-instance deployment would need a shared store to make logout
//! cross-instance.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use uuid::Uuid;

/// `jti` → token expiry (seconds since epoch).
fn blocklist() -> &'static RwLock<HashMap<Uuid, i64>> {
    static BLOCKLIST: OnceLock<RwLock<HashMap<Uuid, i64>>> = OnceLock::new();
    BLOCKLIST.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Drop every entry whose token has already expired (periodic pruning, done
/// lazily on each access so the map never grows past the live revoked set).
fn prune(map: &mut HashMap<Uuid, i64>, now: i64) {
    map.retain(|_, exp| *exp >= now);
}

/// Remember `jti` until `exp`. Tokens issued before the `jti` claim existed
/// carry a nil id and cannot be revoked, so they are ignored.
pub fn revoke(jti: Uuid, exp: usize) {
    if jti.is_nil() {
        return;
    }
    let now = chrono::Utc::now().timestamp();
    let mut map = blocklist().write().expect("revocation list poisoned");
    prune(&mut map, now);
    map.insert(jti, exp as i64);
}

/// `true` when the token's `jti` has been revoked and has not yet expired.
/// An expired entry is treated as absent (the token is dead anyway).
pub fn is_revoked(jti: Uuid) -> bool {
    if jti.is_nil() {
        return false;
    }
    let now = chrono::Utc::now().timestamp();
    let mut map = blocklist().write().expect("revocation list poisoned");
    prune(&mut map, now);
    map.contains_key(&jti)
}
