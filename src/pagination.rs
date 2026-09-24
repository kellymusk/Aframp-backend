use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Opaque cursor for keyset pagination, encoding the `(created_at, id)` of
/// the last row seen. Using both columns (rather than `created_at` alone)
/// keeps the cursor stable when multiple rows share the same timestamp.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

impl Cursor {
    pub fn encode(&self) -> String {
        format!("{}_{}", self.created_at.to_rfc3339(), self.id)
    }

    pub fn decode(raw: &str) -> Option<Cursor> {
        let (ts, id) = raw.rsplit_once('_')?;
        let created_at = DateTime::parse_from_rfc3339(ts).ok()?.with_timezone(&Utc);
        let id = Uuid::parse_str(id).ok()?;
        Some(Cursor { created_at, id })
    }
}

/// Wraps a page of results with the cursor to request the next page.
/// `next_cursor` is `None` once the caller has reached the end of the set.
#[derive(serde::Serialize)]
pub struct Page<T> {
    pub data: Vec<T>,
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    pub fn new(mut data: Vec<T>, limit: i64, cursor_of: impl Fn(&T) -> Cursor) -> Page<T> {
        let next_cursor = if data.len() as i64 > limit {
            data.pop();
            data.last().map(|last| cursor_of(last).encode())
        } else {
            None
        };
        Page { data, next_cursor }
    }
}
