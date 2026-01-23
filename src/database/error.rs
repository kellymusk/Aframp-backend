use std::fmt;

/// Custom database error type for Aframp
#[derive(Debug, Clone)]
pub enum DatabaseErrorKind {
    /// Connection pool is exhausted
    PoolExhausted,
    /// Connection timeout
    ConnectionTimeout,
    /// Record not found
    NotFound {
        entity: String,
        id: String,
    },
    /// Unique constraint violation (e.g., duplicate key)
    UniqueConstraintViolation {
        column: String,
        value: String,
    },
    /// Foreign key constraint violation
    ForeignKeyViolation {
        table: String,
        column: String,
    },
    /// Query execution error
    QueryError {
        message: String,
    },
    /// Transaction error
    TransactionError {
        message: String,
    },
    /// Database connection error
    ConnectionError {
        message: String,
    },
    /// Insufficient balance or quota
    InsufficientBalance {
        available: String,
        required: String,
    },
    /// Trustline not found
    TrustlineNotFound {
        account: String,
        asset: String,
    },
    /// Configuration error
    ConfigError {
        message: String,
    },
    /// Unknown error
    Unknown {
        message: String,
    },
}

/// Result type for database operations
pub type DbResult<T> = Result<T, DatabaseError>;

#[derive(Debug, Clone)]
pub struct DatabaseError {
    pub kind: DatabaseErrorKind,
    pub context: Option<String>,
    pub is_retryable: bool,
}

impl DatabaseError {
    pub fn new(kind: DatabaseErrorKind) -> Self {
        let is_retryable = matches!(
            kind,
            DatabaseErrorKind::ConnectionTimeout
                | DatabaseErrorKind::PoolExhausted
                | DatabaseErrorKind::ConnectionError { .. }
        );

        Self {
            kind,
            context: None,
            is_retryable,
        }
    }

    pub fn with_context<S: Into<String>>(mut self, context: S) -> Self {
        self.context = Some(context.into());
        self
    }

    pub fn is_retryable(&self) -> bool {
        self.is_retryable
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self.kind, DatabaseErrorKind::NotFound { .. })
    }

    pub fn is_constraint_violation(&self) -> bool {
        matches!(
            self.kind,
            DatabaseErrorKind::UniqueConstraintViolation { .. }
                | DatabaseErrorKind::ForeignKeyViolation { .. }
        )
    }

    /// Map SQLx error to our custom error type
    pub fn from_sqlx(error: sqlx::Error) -> Self {
        match error {
            sqlx::Error::RowNotFound => Self::new(DatabaseErrorKind::NotFound {
                entity: "Record".to_string(),
                id: "unknown".to_string(),
            }),
            sqlx::Error::PoolTimedOut => {
                Self::new(DatabaseErrorKind::PoolExhausted)
            }
            sqlx::Error::PoolClosed => {
                Self::new(DatabaseErrorKind::ConnectionError {
                    message: "Connection pool is closed".to_string(),
                })
            }
            sqlx::Error::Configuration(msg) => {
                Self::new(DatabaseErrorKind::ConfigError { message: msg.to_string() })
            }
            sqlx::Error::Database(db_err) => {
                // Handle database-specific errors using trait methods
                let code = db_err.code();
                match code.as_deref() {
                    Some("23505") => {
                        // Unique constraint violation (Postgres code)
                        Self::new(DatabaseErrorKind::UniqueConstraintViolation {
                            column: "unknown".to_string(),
                            value: "provided value".to_string(),
                        })
                    }
                    Some("23503") => {
                        // Foreign key constraint violation (Postgres code)
                        Self::new(DatabaseErrorKind::ForeignKeyViolation {
                            table: "unknown".to_string(),
                            column: "unknown".to_string(),
                        })
                    }
                    _ => Self::new(DatabaseErrorKind::QueryError {
                        message: db_err.message().to_string(),
                    }),
                }
            }
            sqlx::Error::Io(io_err) => {
                Self::new(DatabaseErrorKind::ConnectionError {
                    message: io_err.to_string(),
                })
            }
            _ => Self::new(DatabaseErrorKind::Unknown {
                message: error.to_string(),
            }),
        }
    }
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match &self.kind {
            DatabaseErrorKind::PoolExhausted => {
                "Database connection pool exhausted. Please try again.".to_string()
            }
            DatabaseErrorKind::ConnectionTimeout => {
                "Database connection timed out. Please try again.".to_string()
            }
            DatabaseErrorKind::NotFound { entity, id } => {
                format!("{} with ID '{}' not found", entity, id)
            }
            DatabaseErrorKind::UniqueConstraintViolation { column, value } => {
                format!("A record with {} '{}' already exists", column, value)
            }
            DatabaseErrorKind::ForeignKeyViolation { table, column } => {
                format!(
                    "Cannot perform operation: referenced {} in {} does not exist",
                    column, table
                )
            }
            DatabaseErrorKind::QueryError { message } => {
                format!("Database query failed: {}", message)
            }
            DatabaseErrorKind::TransactionError { message } => {
                format!("Transaction failed: {}", message)
            }
            DatabaseErrorKind::ConnectionError { message } => {
                format!("Database connection error: {}", message)
            }
            DatabaseErrorKind::InsufficientBalance {
                available,
                required,
            } => {
                format!(
                    "Insufficient balance. Available: {}, Required: {}",
                    available, required
                )
            }
            DatabaseErrorKind::TrustlineNotFound { account, asset } => {
                format!("Trustline not found for account '{}' and asset '{}'", account, asset)
            }
            DatabaseErrorKind::ConfigError { message } => {
                format!("Database configuration error: {}", message)
            }
            DatabaseErrorKind::Unknown { message } => {
                format!("Unknown database error: {}", message)
            }
        };

        if let Some(context) = &self.context {
            write!(f, "{} ({})", message, context)
        } else {
            write!(f, "{}", message)
        }
    }
}

impl std::error::Error for DatabaseError {}

impl PartialEq for DatabaseError {
    fn eq(&self, other: &Self) -> bool {
        // For testing purposes
        format!("{:?}", self.kind) == format!("{:?}", other.kind)
    }
}