pub mod cookie;
pub mod extractor;
pub mod jwt;
pub mod password;

#[cfg(test)]
mod extractor_tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    use super::extractor::AuthUser;

    async fn whoami(user: AuthUser) -> String {
        user.0
    }

    fn app() -> Router {
        Router::new().route("/whoami", get(whoami))
    }

    #[tokio::test]
    async fn accepts_mixed_case_bearer_prefix() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/whoami")
                    .header("authorization", "bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_missing_credentials() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/whoami")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
pub mod revocation;
