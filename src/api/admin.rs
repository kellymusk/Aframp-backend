use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Html;
use axum::Json;
use futures_util::stream::Stream;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::auth::extractor::AdminUser;
use crate::error::{bad_request, internal, not_found, ApiResult, ErrorCode};
use crate::models::{
    AdminMerchantRow, AdminOverview, AdminPaymentRequestRow, AdminTransactionRow, AdminUserRow,
    AdminWalletRow, AdminWithdrawalRow, ListParams, Merchant,
};
use crate::pagination::{Cursor, Page};
use crate::services::admin;
use crate::services::merchant_webhooks;
use crate::AppState;

/// Query parameters for `GET /admin/users`. Extends the standard cursor
/// pagination with an optional `q` search term matched against email prefix
/// or exact phone number.
#[derive(Debug, Default, serde::Deserialize)]
pub struct AdminUserListParams {
    #[serde(flatten)]
    pub list: ListParams,
    /// Search term: email prefix or exact phone number.
    pub q: Option<String>,
}

pub async fn overview(
    State(state): State<AppState>,
    admin: AdminUser,
) -> ApiResult<Json<AdminOverview>> {
    admin.require_financials()?;
    let overview = admin::overview(&state.db).await.map_err(internal)?;
    Ok(Json(overview))
}

pub async fn users(
    State(state): State<AppState>,
    admin: AdminUser,
    Query(params): Query<AdminUserListParams>,
) -> ApiResult<Json<Page<AdminUserRow>>> {
    admin.require_pii()?;
    let cursor = match params.list.cursor.as_deref() {
        Some(raw) => Some(
            Cursor::decode(raw)
                .ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?,
        ),
        None => None,
    };
    let query = params.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let page = admin::users(&state.db, params.list.admin_limit(), cursor, query)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

pub async fn merchants(
    State(state): State<AppState>,
    admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminMerchantRow>>> {
    admin.require_pii()?;
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(
            Cursor::decode(raw)
                .ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?,
        ),
        None => None,
    };
    let page = admin::merchants(&state.db, params.admin_limit(), cursor)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

pub async fn wallets(
    State(state): State<AppState>,
    admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminWalletRow>>> {
    admin.require_pii()?;
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(
            Cursor::decode(raw)
                .ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?,
        ),
        None => None,
    };
    let page = admin::wallets(&state.db, params.admin_limit(), cursor)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

pub async fn transactions(
    State(state): State<AppState>,
    admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminTransactionRow>>> {
    admin.require_financials()?;
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(
            Cursor::decode(raw)
                .ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?,
        ),
        None => None,
    };
    let page = admin::transactions(&state.db, params.admin_limit(), cursor)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

pub async fn withdrawals(
    State(state): State<AppState>,
    admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminWithdrawalRow>>> {
    admin.require_financials()?;
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(
            Cursor::decode(raw)
                .ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?,
        ),
        None => None,
    };
    let page = admin::withdrawals(&state.db, params.admin_limit(), cursor)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

pub async fn payment_requests(
    State(state): State<AppState>,
    admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminPaymentRequestRow>>> {
    admin.require_financials()?;
    let cursor = match params.cursor.as_deref() {
        Some(raw) => Some(
            Cursor::decode(raw)
                .ok_or_else(|| bad_request(ErrorCode::InvalidParameters, "invalid cursor"))?,
        ),
        None => None,
    };
    let page = admin::payment_requests(&state.db, params.admin_limit(), cursor)
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

/// `GET /admin/webhooks/health` — list merchants whose webhook endpoint has
/// failed `threshold` or more consecutive deliveries. Surfaces silent webhook
/// outages so operators can alert the merchant.
pub async fn webhooks_health(
    State(state): State<AppState>,
    admin: AdminUser,
) -> ApiResult<Json<Vec<merchant_webhooks::WebhookHealthRow>>> {
    admin.require_pii()?;
    let rows = merchant_webhooks::unhealthy_merchants(&state.db)
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
    let found = admin::unlock_user(&state.db, id).await.map_err(internal)?;
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
        let digest = <sha2::Sha256 as sha2::Dig

/* … truncated 1933 chars — edit only what you need near the top … */
