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

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift PRNG so the fuzz test needs no extra crates
    /// and runs the same corpus on every CI invocation.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn byte(&mut self) -> u8 {
            (self.next() & 0xff) as u8
        }
    }

    /// Fuzz `Cursor::decode` with arbitrary byte strings to ensure it never
    /// panics and always returns `Some` or `None` for any `&str` input.
    #[test]
    fn decode_never_panics_on_arbitrary_input() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let alphabet: &[u8] = b"0123456789abcdefABCDEF-:TZ._+ ";

        for _ in 0..10_000 {
            let len = (rng.next() % 64) as usize;
            let mut buf = String::with_capacity(len);
            for _ in 0..len {
                let b = if rng.next() % 2 == 0 {
                    rng.byte()
                } else {
                    alphabet[(rng.next() as usize) % alphabet.len()]
                };
                // Only push valid UTF-8; skip stray continuation bytes.
                if b < 0x80 {
                    buf.push(b as char);
                }
            }
            // Must not panic; result is either Some or None.
            let _ = Cursor::decode(&buf);
        }
    }

    #[test]
    fn decode_roundtrips_encoded_cursor() {
        let cursor = Cursor {
            created_at: Utc::now(),
            id: Uuid::nil(),
        };
        let decoded = Cursor::decode(&cursor.encode()).expect("encoded cursor must decode");
        assert_eq!(decoded.id, cursor.id);
        assert_eq!(decoded.created_at, cursor.created_at);
    }

    #[test]
    fn decode_rejects_malformed_input() {
        assert!(Cursor::decode("").is_none());
        assert!(Cursor::decode("no-underscore").is_none());
        assert!(Cursor::decode("not-a-date_not-a-uuid").is_none());
    }
}
