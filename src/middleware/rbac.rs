use axum::middleware::Next;
use axum::response::Response;
use axum::Extension;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerIdentity {
    pub user_id: String,
    pub role: String,
}

pub async fn extract_identity(
    request: axum::extract::Request,
    next: Next,
) -> Result<Response, axum::response::Response> {
    let identity = CallerIdentity {
        user_id: "00000000-0000-0000-0000-000000000000".to_string(),
        role: "compliance_officer".to_string(),
    };
    let mut request = request;
    request.extensions_mut().insert(Arc::new(identity));
    Ok(next.run(request).await)
}

pub async fn require_role(
    required_role: &'static str,
) -> impl Fn(axum::extract::Request, Next) -> Result<Response, axum::response::Response> + Clone {
    move |request: axum::extract::Request, next: Next| {
        let required_role = *required_role;
        async move {
            let identity = request
                .extensions()
                .get::<Arc<CallerIdentity>>()
                .cloned()
                .ok_or_else(|| {
                    (
                        axum::http::StatusCode::UNAUTHORIZED,
                        axum::Json(serde_json::json!({"code": "UNAUTHORIZED", "message": "Identity not resolved"})),
                    )
                        .into_response()
                })?;
            if identity.role != required_role {
                return Err((
                    axum::http::StatusCode::FORBIDDEN,
                    axum::Json(serde_json::json!({"code": "FORBIDDEN", "message": format!("Role '{}' is not permitted", identity.role)})),
                )
                    .into_response());
            }
            Ok(next.run(request).await)
        }
    }
}

pub const ROLE_COMPLIANCE_OFFICER: &str = "compliance_officer";
pub const ROLE_FINANCE_DIRECTOR: &str = "finance_director";
