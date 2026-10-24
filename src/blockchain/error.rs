/// Typed error enum for the blockchain module.
///
/// Using a concrete enum instead of `String` lets callers react differently
/// to transient vs. permanent failures and lets the worker log the right
/// severity level for each category.
#[derive(Debug, thiserror::Error)]
pub enum BlockchainError {
    /// A network-level failure talking to Horizon (transient — retry is
    /// reasonable).  Logged at `warn` because the next poll cycle will retry.
    #[error("horizon HTTP error: {0}")]
    HttpError(String),

    /// A response arrived but its contents could not be parsed (e.g. an
    /// unexpected Horizon response shape or an unparseable amount string).
    /// Logged at `error` because retrying the same data will not fix it.
    #[error("parse error: {0}")]
    ParseError(String),

    /// A database operation failed.  Logged at `error` because this indicates
    /// a problem with the platform's own storage layer, not with Horizon.
    #[error("database error: {0}")]
    DatabaseError(String),

    /// A wallet address was not found in the local database.  Logged at
    /// `warn` because this can happen transiently during wallet creation.
    #[error("not found: {0}")]
    NotFound(String),
}

impl From<sqlx::Error> for BlockchainError {
    fn from(e: sqlx::Error) -> Self {
        BlockchainError::DatabaseError(e.to_string())
    }
}
