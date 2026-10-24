use axum::extract::{Path, Query, State};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::extract::{Path, Query, State};
use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Html;
use axum::Json;
use futures::stream::{self, Stream};
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

use crate::auth::extractor::AdminUser;
use crate::error::{internal, ApiError, ApiResult};
use crate::models::{
    AdminKycDecisionRequest, AdminMerchantRow, AdminOverview, AdminPaymentRequestRow,
    AdminTransactionRow, AdminUserRow, AdminWalletRow, AdminWithdrawalRow,
use crate::error::{internal, not_found, ApiResult, ErrorCode};
use crate::models::{
    AdminMerchantRow, AdminOverview, AdminPaymentRequestRow, AdminTransactionRow, AdminUserRow,
    AdminWalletRow, AdminWithdrawalRow, ListParams,
    AdminWalletRow, AdminWithdrawalRow, Merchant,
};
use crate::pagination::{Cursor, Page};
use crate::services::admin;
use crate::services::merchant;
use crate::AppState;

#[derive(Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

fn limit(params: &ListParams) -> i64 {
    params.limit.unwrap_or(100).clamp(1, 500)
}

/// Financial data (balances, withdrawals) is restricted to `super_admin` and
/// `finance` admins. Other admin roles get a 403.
fn require_finance(admin: &AdminUser) -> ApiResult<()> {
    if admin.has_role(&["super_admin", "finance"]) {
        Ok(())
    } else {
        Err(crate::error::ApiError::Forbidden)
    }
}

/// PII-heavy data (user emails) is restricted to `super_admin` and `support`
/// admins. Other admin roles get a 403.
fn require_pii(admin: &AdminUser) -> ApiResult<()> {
    if admin.has_role(&["super_admin", "support"]) {
        Ok(())
    } else {
        Err(crate::error::ApiError::Forbidden)
/// Validate the `X-CSRF-Token` header on state-changing admin requests.
///
/// The dashboard embeds a per-session token in a `<meta name="csrf-token">`
/// tag and echoes it back on every mutating fetch. When `COOKIE_SAME_SITE=none`
/// the session cookie rides on cross-site requests, so this header check is what
/// stops a forged cross-site POST from reaching a mutating route.
fn require_csrf(headers: &HeaderMap, admin: &AdminUser) -> ApiResult<()> {
    let provided = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if provided.is_empty() || provided != admin.csrf_token {
        return Err(crate::error::ApiError::forbidden("invalid or missing CSRF token"));
    }
    Ok(())
fn cursor(params: &ListParams) -> ApiResult<Option<Cursor>> {
    match params.cursor.as_deref() {
        Some(raw) => Cursor::decode(raw)
            .map(Some)
            .map_err(|_| crate::error::ApiError::bad_request("invalid cursor")),
        None => Ok(None),
    }
}

pub async fn overview(
    State(state): State<AppState>,
    admin: AdminUser,
) -> ApiResult<Json<AdminOverview>> {
    require_finance(&admin)?;
    let overview = admin::overview(&state.db).await.map_err(internal)?;
    Ok(Json(overview))
}

pub async fn users(
    State(state): State<AppState>,
    admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminUserRow>>> {
    let page = admin::users(&state.db, limit(&params), cursor(&params)?)
        .await
        .map_err(internal)?;
    Ok(Json(page))
) -> ApiResult<Json<Vec<AdminUserRow>>> {
    require_pii(&admin)?;
    let rows = admin::users(&state.db, limit(&params)).await.map_err(internal)?;
    let rows = admin::users(&state.db, params.admin_limit()).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn merchants(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminMerchantRow>>> {
    let page = admin::merchants(&state.db, limit(&params), cursor(&params)?)
        .await
        .map_err(internal)?;
    Ok(Json(page))
) -> ApiResult<Json<Vec<AdminMerchantRow>>> {
    let rows = admin::merchants(&state.db, params.admin_limit()).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn suspend_merchant(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<AdminMerchantRow>> {
    let row = admin::set_merchant_active(&state.db, id, false)
        .await
        .map_err(internal)?;
    Ok(Json(row))
}

pub async fn activate_merchant(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<AdminMerchantRow>> {
    let row = admin::set_merchant_active(&state.db, id, true)
        .await
        .map_err(internal)?;
pub async fn set_merchant_kyc(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i64>,
    Json(payload): Json<AdminKycDecisionRequest>,
) -> ApiResult<Json<AdminMerchantRow>> {
    let status = payload.status.trim().to_ascii_lowercase();
    if status != "verified" && status != "rejected" {
        return Err(ApiError::BadRequest(
            "status must be either 'verified' or 'rejected'".to_string(),
        ));
    }

    let row = admin::set_merchant_kyc(&state.db, id, &status)
        .await
        .map_err(internal)?
        .ok_or_else(|| ApiError::NotFound("merchant not found".to_string()))?;

    Ok(Json(row))
}

pub async fn wallets(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminWalletRow>>> {
    let page = admin::wallets(&state.db, limit(&params), cursor(&params)?)
        .await
        .map_err(internal)?;
    Ok(Json(page))
) -> ApiResult<Json<Vec<AdminWalletRow>>> {
    let rows = admin::wallets(&state.db, params.admin_limit()).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn transactions(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminTransactionRow>>> {
    let page = admin::transactions(&state.db, limit(&params), cursor(&params)?)
        .await
        .map_err(internal)?;
    Ok(Json(page))
) -> ApiResult<Json<Vec<AdminTransactionRow>>> {
    let rows = admin::transactions(&state.db, params.admin_limit()).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn withdrawals(
    State(state): State<AppState>,
    admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminWithdrawalRow>>> {
    let page = admin::withdrawals(&state.db, limit(&params), cursor(&params)?)
        .await
        .map_err(internal)?;
    Ok(Json(page))
) -> ApiResult<Json<Vec<AdminWithdrawalRow>>> {
    require_finance(&admin)?;
    let rows = admin::withdrawals(&state.db, limit(&params)).await.map_err(internal)?;
    let rows = admin::withdrawals(&state.db, params.admin_limit()).await.map_err(internal)?;
    Ok(Json(rows))
}

pub async fn payment_requests(
    State(state): State<AppState>,
    _admin: AdminUser,
    Query(params): Query<ListParams>,
) -> ApiResult<Json<Page<AdminPaymentRequestRow>>> {
    let page = admin::payment_requests(&state.db, limit(&params), cursor(&params)?)
) -> ApiResult<Json<Vec<AdminPaymentRequestRow>>> {
    let rows = admin::payment_requests(&state.db, params.admin_limit())
        .await
        .map_err(internal)?;
    Ok(Json(page))
}

/// Server-Sent Events stream of live admin activity. Protected by `AdminUser`,
/// so only authenticated admins can subscribe. Events are broadcast from
/// `AppState::events` and forwarded to every connected client.
pub async fn events(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.events.subscribe();
    let stream = BroadcastStream::new(receiver).filter_map(|msg| match msg {
        Ok(event) => Some(Ok(Event::default().event(event.kind).data(event.data))),
        // A lagging client missed some events; skip rather than terminate.
        Err(_) => None,
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
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

#[derive(Deserialize)]
pub struct KycSubmission {
    pub bvn: Option<String>,
    pub cac_number: Option<String>,
}

/// `PUT /merchants/kyc` — a merchant submits basic KYC info. On success the
/// merchant's `kyc_status` moves to `pending` and the updated record is
/// returned.
pub async fn submit_kyc(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<KycSubmission>,
) -> ApiResult<Json<Merchant>> {
    let merchant_id = auth
        .merchant_id
        .ok_or_else(|| forbidden("only merchants can submit KYC"))?;

    let bvn = payload
        .bvn
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let cac_number = payload
        .cac_number
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if bvn.is_none() && cac_number.is_none() {
        return Err(bad_request("provide at least a BVN or CAC number"));
    }

    if let Some(bvn) = bvn {
        if bvn.len() != 11 || !bvn.chars().all(|c| c.is_ascii_digit()) {
            return Err(bad_request("BVN must be exactly 11 digits"));
        }
    }

    if let Some(cac_number) = cac_number {
        if cac_number.len() < 2 || cac_number.len() > 32 {
            return Err(bad_request("CAC number must be between 2 and 32 characters"));
        }
        if !cac_number
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '/')
        {
            return Err(bad_request("CAC number contains invalid characters"));
        }
    }

    let merchant = merchant::submit_kyc(&state.db, merchant_id, bvn, cac_number)
        .await
        .map_err(internal)?;
    Ok(Json(merchant))
}

#[derive(Deserialize)]
pub struct KycDecision {
    pub status: String,
}

/// `PATCH /admin/merchants/{id}/kyc` — an admin approves or rejects a
/// merchant's KYC submission.
pub async fn update_merchant_kyc(
    State(state): State<AppState>,
    _admin: AdminUser,
    Path(id): Path<i64>,
    Json(payload): Json<KycDecision>,
) -> ApiResult<Json<Merchant>> {
    let status = payload.status.trim().to_ascii_lowercase();
    if status != "verified" && status != "rejected" {
        return Err(bad_request("status must be 'verified' or 'rejected'"));
    }

    let merchant = merchant::set_kyc_status(&state.db, id, &status)
        .await
        .map_err(internal)?
        .ok_or_else(|| not_found("merchant not found"))?;
    Ok(Json(merchant))
}

/// Static dashboard shell. Unauthenticated by design — it's markup and JS with
/// no data baked in. It logs in through the normal `/login` endpoint (setting
/// the same HttpOnly session cookie a browser client would get) and every data
/// fetch after that rides the cookie same-origin; nothing here bypasses the
/// `AdminUser` check on the JSON routes below.
///
/// The shell also exposes the per-session CSRF token via a `<meta>` tag so the
/// dashboard JS can echo it back in the `X-CSRF-Token` header on mutating calls.
pub async fn dashboard(admin: AdminUser) -> Html<String> {
    let shell = include_str!("admin_dashboard.html");
    let meta = format!(
        "<meta name=\"csrf-token\" content=\"{}\">",
        admin.csrf_token
    );
    let html = shell.replacen("</head>", &format!("{}\n</head>", meta), 1);
    Html(html)
pub async fn dashboard() -> impl axum::response::IntoResponse {
    use axum::http::header;
    (
        [
            (header::CONTENT_SECURITY_POLICY, "default-src 'self'"),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "strict-origin-when-cross-origin"),
        ],
        Html(include_str!("admin_dashboard.html")),
    )
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::auth::jwt;
    use crate::routes::build_router;
    use crate::AppState;

    async fn test_state() -> AppState {
        AppState::for_tests().await
    }

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("read body");
        serde_json::from_slice(&bytes).expect("parse json")
    }

    async fn post_json(
        app: &axum::Router,
        uri: &str,
        payload: serde_json::Value,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn get_with_token(
        app: &axum::Router,
        uri: &str,
        token: &str,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn admin_login_with_phone_returns_challenge_not_token() {
        let state = test_state().await;
        let app = build_router(state.clone());

        let admin = crate::services::users::create_admin_with_phone(
            &state.db,
            "admin-otp@example.com",
            "+15550000001",
        )
        .await
        .expect("create admin");

        let resp = post_json(
            &app,
            "/login",
            serde_json::json!({
                "email": admin.email,
                "password": "password123",
            }),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_json(resp).await;
        assert!(
            body.get("challenge_id").is_some(),
            "expected challenge response, got {body:?}"
        );
        assert!(
            body.get("token").is_none(),
            "admin with phone must not receive a session token directly"
        );
    }

    #[tokio::test]
    async fn admin_otp_verification_yields_admin_token() {
        let state = test_state().await;
        let app = build_router(state.clone());

        let admin = crate::services::users::create_admin_with_phone(
            &state.db,
            "admin-otp2@example.com",
            "+15550000002",
        )
        .await
        .expect("create admin");

        let login = post_json(
            &app,
            "/login",
            serde_json::json!({
                "email": admin.email,
                "password": "password123",
            }),
        )
        .await;
        let login_body = body_json(login).await;
        let challenge_id = login_body
            .get("challenge_id")
            .and_then(|v| v.as_str())
            .expect("challenge_id")
            .to_string();

        let code = crate::services::otp::peek_code_for_tests(&state.db, &challenge_id)
            .await
            .expect("otp code");

        let verify = post_json(
            &app,
            "/verify-otp",
            serde_json::json!({
                "challenge_id": challenge_id,
                "code": code,
            }),
        )
        .await;
        assert_eq!(verify.status(), StatusCode::OK);
        let verify_body = body_json(verify).await;
        let token = verify_body
            .get("token")
            .and_then(|v| v.as_str())
            .expect("token")
            .to_string();

        let claims = jwt::decode(&token).expect("decode token");
        assert!(claims.is_admin, "admin token must carry is_admin: true");
    }

    #[tokio::test]
    async fn admin_token_can_access_overview() {
        let state = test_state().await;
        let app = build_router(state.clone());

        let admin = crate::services::users::create_admin_with_phone(
            &state.db,
            "admin-otp3@example.com",
            "+15550000003",
        )
        .await
        .expect("create admin");

        let token = jwt::encode(&admin.id, true).expect("encode admin token");
        let resp = get_with_token(&app, "/admin/overview", &token).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn non_admin_token_is_forbidden_from_overview() {
        let state = test_state().await;
        let app = build_router(state.clone());

        let user = crate::services::users::create(
            &state.db,
            "plain-user@example.com",
            "password123",
        )
        .await
        .expect("create user");

        let token = jwt::encode(&user.id, false).expect("encode user token");
        let resp = get_with_token(&app, "/admin/overview", &token).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
