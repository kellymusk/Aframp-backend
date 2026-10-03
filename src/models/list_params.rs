use serde::Deserialize;

/// Default page size for merchant-facing list endpoints.
pub const MERCHANT_DEFAULT_LIMIT: i64 = 50;
/// Maximum page size for merchant-facing list endpoints.
pub const MERCHANT_MAX_LIMIT: i64 = 200;

/// Default page size for admin list endpoints.
pub const ADMIN_DEFAULT_LIMIT: i64 = 100;
/// Maximum page size for admin list endpoints.
pub const ADMIN_MAX_LIMIT: i64 = 500;

/// Query parameters shared by all list endpoints (`?limit=`).
///
/// Use [`ListParams::merchant_limit`] for merchant routes (default 50, max 200)
/// and [`ListParams::admin_limit`] for admin routes (default 100, max 500).
#[derive(Debug, Deserialize)]
pub struct ListParams {
    pub limit: Option<i64>,
    pub cursor: Option<String>,
}

impl ListParams {
    /// Clamped limit for merchant-facing list endpoints (default 50, 1–200).
    pub fn merchant_limit(&self) -> i64 {
        self.limit
            .unwrap_or(MERCHANT_DEFAULT_LIMIT)
            .clamp(1, MERCHANT_MAX_LIMIT)
    }

    /// Clamped limit for admin list endpoints (default 100, 1–500).
    pub fn admin_limit(&self) -> i64 {
        self.limit
            .unwrap_or(ADMIN_DEFAULT_LIMIT)
            .clamp(1, ADMIN_MAX_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(limit: Option<i64>) -> ListParams {
        ListParams { limit, cursor: None }
    }

    // --- merchant_limit ---

    #[test]
    fn merchant_limit_default_when_none() {
        assert_eq!(params(None).merchant_limit(), MERCHANT_DEFAULT_LIMIT);
    }

    #[test]
    fn merchant_limit_uses_provided_value() {
        assert_eq!(params(Some(10)).merchant_limit(), 10);
    }

    #[test]
    fn merchant_limit_clamps_to_max() {
        assert_eq!(params(Some(999)).merchant_limit(), MERCHANT_MAX_LIMIT);
    }

    #[test]
    fn merchant_limit_clamps_to_min() {
        assert_eq!(params(Some(0)).merchant_limit(), 1);
    }

    #[test]
    fn merchant_limit_clamps_negative() {
        assert_eq!(params(Some(-10)).merchant_limit(), 1);
    }

    #[test]
    fn merchant_limit_at_exact_max() {
        assert_eq!(params(Some(200)).merchant_limit(), MERCHANT_MAX_LIMIT);
    }

    #[test]
    fn merchant_limit_at_exact_min() {
        assert_eq!(params(Some(1)).merchant_limit(), 1);
    }

    // --- admin_limit ---

    #[test]
    fn admin_limit_default_when_none() {
        assert_eq!(params(None).admin_limit(), ADMIN_DEFAULT_LIMIT);
    }

    #[test]
    fn admin_limit_uses_provided_value() {
        assert_eq!(params(Some(50)).admin_limit(), 50);
    }

    #[test]
    fn admin_limit_clamps_to_max() {
        assert_eq!(params(Some(9999)).admin_limit(), ADMIN_MAX_LIMIT);
    }

    #[test]
    fn admin_limit_clamps_to_min() {
        assert_eq!(params(Some(0)).admin_limit(), 1);
    }

    #[test]
    fn admin_limit_clamps_negative() {
        assert_eq!(params(Some(-5)).admin_limit(), 1);
    }

    #[test]
    fn admin_limit_at_exact_max() {
        assert_eq!(params(Some(500)).admin_limit(), ADMIN_MAX_LIMIT);
    }

    #[test]
    fn admin_limit_at_exact_min() {
        assert_eq!(params(Some(1)).admin_limit(), 1);
    }
}
