use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Html;
use axum::Json;
use futures_util::stream::Stream;
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::auth::extractor::AdminUser;
use crate::error::{internal, not_found, ApiResult, ErrorCode};
use crate::models::{
    AdminMerchantRow, AdminOverview, AdminPaymentRequestRow, AdminTransactionRow, AdminUserRow,
    AdminWalletRow, AdminWithdrawalRow, ListParams, Merchant,
};
use crate::services::admin;
use crate::AppState;

pub async fn overview(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> ApiResult<Json<AdminOverview>> {
    let overview = admin::overview(&state.db).await.map_err(internal)?;
    Ok(Json(overview))
}

pub async fn users(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminUserRow>>> {
    let rows = admin::users(&state.db, params.admin_limit()).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn merchants(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminMerchantRow>>> {
    let rows = admin::merchants(&state.db, params.admin_limit()).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn wallets(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminWalletRow>>> {
    let rows = admin::wallets(&state.db, params.admin_limit()).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn transactions(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminTransactionRow>>> {
    let rows = admin::transactions(&state.db, params.admin_limit()).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn withdrawals(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminWithdrawalRow>>> {
    let rows = admin::withdrawals(&state.db, params.admin_limit()).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn payment_requests(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Vec<AdminPaymentRequestRow>>> {
    let rows = admin::payment_requests(&state.db, params.admin_limit())
        .await
        .map_err(internal)?;
    Ok(Json(rows))
}

/// Server-Sent Events stream of live admin activity. Protected by `AdminUser`,
/// so only authenticated admins can subscribe. Events are broadcast from
/// `AppState::admin_events` and forwarded to every connected client.
pub async fn events(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.admin_events.subscribe();
    let stream = BroadcastStream::new(receiver).filter_map(|msg| match msg {
        Ok(event) => Some(Ok(Event::default().event(event.name()).data("{}"))),
        // A lagging client missed some events; skip rather than terminate.
        Err(_) => None,
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// `POST /admin/merchants/{id}/suspend` — suspend a merchant account.
/// The merchant will receive 403 on /login, /payment-requests, and /withdraw
/// until unsuspended.
pub async fn suspend_merchant(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Merchant>> {
    let merchant = admin::suspend_merchant(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;
    Ok(Json(merchant))
}

/// `POST /admin/merchants/{id}/unsuspend` — reinstate a suspended merchant.
pub async fn unsuspend_merchant(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Merchant>> {
    let merchant = admin::unsuspend_merchant(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found(ErrorCode::MerchantNotFound, "merchant not found"))?;
    Ok(Json(merchant))
}

/// `POST /admin/users/{id}/unlock` — clear an account lockout applied by the
/// failed-login counter. Also resets `failed_login_count` to 0.
pub async fn unlock_user(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<Uuid>,
) -> ApiResult<axum::http::StatusCode> {
    let found = admin::unlock_user(&state.db, id)
        .await
        .map_err(internal)?;
    if found {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(not_found(ErrorCode::UserNotFound, "user not found"))
    }
}

/// Static dashboard shell. Unauthenticated by design — it's markup and JS with
/// no data baked in. It logs in through the normal `/login` endpoint (setting
/// the same HttpOnly session cookie a browser client would get) and every data
/// fetch after that rides the cookie same-origin; nothing here bypasses the
/// `AdminUser` check on the JSON routes below.
pub async fn dashboard() -> impl axum::response::IntoResponse {
    use axum::http::header;
    (
        [
            (header::CONTENT_SECURITY_POLICY, dashboard_csp()),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "strict-origin-when-cross-origin"),
        ],
        Html(DASHBOARD_HTML),
    )
}

const DASHBOARD_HTML: &str = include_str!("admin_dashboard.html");

/// The dashboard is a single self-contained page with one inline `<script>`,
/// so the policy allows exactly that script by hash (any injected script is
/// blocked) and inline styles, and nothing from other origins.
fn dashboard_csp() -> &'static str {
    static CSP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CSP.get_or_init(|| {
        let script = inline_script(DASHBOARD_HTML).expect("admin dashboard has an inline script");
        let digest = <sha2::Sha256 as sha2::Digest>::digest(script.as_bytes());
        format!(
            "default-src 'self'; script-src 'sha256-{}'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            base64_std(&digest)
        )
    })
}

fn inline_script(html: &str) -> Option<&str> {
    let start = html.find("<script>")? + "<script>".len();
    let end = start + html[start..].find("</script>")?;
    Some(&html[start..end])
}

fn base64_std(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_std_matches_rfc4648_vectors() {
        for (plain, encoded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64_std(plain.as_bytes()), encoded);
        }
    }

    #[test]
    fn dashboard_csp_pins_the_inline_script_hash() {
        let csp = dashboard_csp();
        assert!(csp.contains("script-src 'sha256-"), "{csp}");
        assert!(!csp.contains("'unsafe-inline'; img") || csp.contains("style-src 'self' 'unsafe-inline'"));
        assert!(!csp.contains("script-src 'unsafe-inline'"), "inline scripts must not be allowed wholesale");
    }
}
