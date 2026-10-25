use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Opaque cursor for keyset pagination, encoding the `(created_at, id)` of
/// the last row seen. Using both columns (rather than `created_at` alone)
/// keeps the cursor stable when multiple rows share the same timestamp.
///
/// On the wire it's unpadded base64url of `"<rfc3339>_<uuid>"`, so clients
/// treat it as an opaque token rather than something to construct.
#[derive(Debug, Clone, Copy)]
pub struct Cursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

impl Cursor {
    pub fn encode(&self) -> String {
        base64url_encode(format!("{}_{}", self.created_at.to_rfc3339(), self.id).as_bytes())
    }

    pub fn decode(raw: &str) -> Option<Cursor> {
        let plain = String::from_utf8(base64url_decode(raw)?).ok()?;
        let (ts, id) = plain.rsplit_once('_')?;
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
/// Parses an optional raw cursor string into a `Cursor`, returning `None`
/// when the input is absent or malformed. Admin list endpoints use this to
/// translate the `?cursor=` query parameter into a keyset position.
pub fn parse_cursor(raw: Option<&str>) -> Option<Cursor> {
    raw.and_then(Cursor::decode)
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use std::collections::HashSet;

    /// Minimal row shape mirroring what the repositories paginate over.
    #[derive(Debug, Clone)]
    struct Row {
        created_at: DateTime<Utc>,
        id: Uuid,
    }

    impl Row {
        fn cursor(&self) -> Cursor {
            Cursor {
                created_at: self.created_at,
                id: self.id,
            }
        }
    }

    /// Builds 25 rows in descending `(created_at, id)` order, the same
    /// ordering the keyset queries use. Several rows share a timestamp to
    /// exercise the tie-breaker on `id`.
    fn seed_rows() -> Vec<Row> {
        let base = Utc::now();
        let mut rows: Vec<Row> = (0..25)
            .map(|i| Row {
                // Groups of 5 share a timestamp.
                created_at: base - Duration::seconds((i / 5) as i64),
                id: Uuid::new_v4(),
            })
            .collect();
        rows.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        rows
    }

    /// Emulates the keyset query: return rows strictly after `cursor` in
    /// descending order, fetching `limit + 1` to detect a following page.
    fn fetch_page(rows: &[Row], cursor: Option<Cursor>, limit: i64) -> Page<Row> {
        let start = match cursor {
            None => 0,
            Some(c) => rows
                .iter()
                .position(|r| {
                    r.created_at < c.created_at
                        || (r.created_at == c.created_at && r.id < c.id)
                })
                .unwrap_or(rows.len()),
        };
        let window: Vec<Row> = rows
            .iter()
            .skip(start)
            .take((limit + 1) as usize)
            .cloned()
            .collect();
        Page::new(window, limit, |r| r.cursor())
    }

    fn assert_paginates_correctly(rows: Vec<Row>) {
        let limit = 10;

        // Page 1: 10 items, non-null cursor.
        let page1 = fetch_page(&rows, None, limit);
        assert_eq!(page1.data.len(), 10, "page 1 should have 10 items");
        let cursor1 = page1.next_cursor.expect("page 1 should have a next cursor");

        // Page 2: 10 items, different from page 1, non-null cursor.
        let page2 = fetch_page(&rows, Cursor::decode(&cursor1), limit);
        assert_eq!(page2.data.len(), 10, "page 2 should have 10 items");
        let cursor2 = page2.next_cursor.expect("page 2 should have a next cursor");
        let page1_ids: HashSet<Uuid> = page1.data.iter().map(|r| r.id).collect();
        assert!(
            page2.data.iter().all(|r| !page1_ids.contains(&r.id)),
            "page 2 must not repeat page 1 rows"
        );

        // Page 3: 5 items, null cursor (end of set).
        let page3 = fetch_page(&rows, Cursor::decode(&cursor2), limit);
        assert_eq!(page3.data.len(), 5, "page 3 should have 5 items");
        assert!(page3.next_cursor.is_none(), "page 3 should be the last page");

        // No duplicates or skips across all pages.
        let mut all_ids: Vec<Uuid> = Vec::new();
        all_ids.extend(page1.data.iter().map(|r| r.id));
        all_ids.extend(page2.data.iter().map(|r| r.id));
        all_ids.extend(page3.data.iter().map(|r| r.id));
        let unique: HashSet<Uuid> = all_ids.iter().copied().collect();
        assert_eq!(all_ids.len(), 25, "all 25 rows should be returned");
        assert_eq!(unique.len(), 25, "no row should be duplicated");
    }

    #[test]
    fn cursor_pagination_transactions() {
        assert_paginates_correctly(seed_rows());
    }

    #[test]
    fn cursor_pagination_withdrawals() {
        assert_paginates_correctly(seed_rows());
    }

    #[test]
    fn cursor_pagination_payment_requests() {
        assert_paginates_correctly(seed_rows());
    }

    #[test]
    fn cursor_roundtrip_preserves_position() {
        let row = seed_rows().into_iter().next().unwrap();
        let decoded = Cursor::decode(&row.cursor().encode()).expect("cursor should decode");
        assert_eq!(decoded.created_at, row.created_at);
        assert_eq!(decoded.id, row.id);
const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Unpadded base64url (RFC 4648 §5). Small enough to keep here rather than
/// add a dependency just for cursors.
fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(BASE64URL[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
        }
    }
    out
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let values = input
        .bytes()
        .map(|c| BASE64URL.iter().position(|&b| b == c).map(|v| v as u32))
        .collect::<Option<Vec<u32>>>()?;
    if values.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(values.len() * 3 / 4);
    for chunk in values.chunks(4) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, v)| acc | (v << (18 - 6 * i)));
        for i in 0..chunk.len() - 1 {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_and_is_opaque() {
        let cursor = Cursor {
            created_at: DateTime::parse_from_rfc3339("2026-09-26T12:34:56.789Z").unwrap().with_timezone(&Utc),
            id: Uuid::parse_str("5f0c7c1e-2d4b-4a8e-9a55-0c1f1b2a3d4e").unwrap(),
        };
        let encoded = cursor.encode();
        assert!(encoded.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert!(!encoded.contains("2026"), "cursor must not expose the raw timestamp");

        let decoded = Cursor::decode(&encoded).expect("round trip");
        assert_eq!(decoded.created_at, cursor.created_at);
        assert_eq!(decoded.id, cursor.id);
    }

    #[test]
    fn cursor_round_trips_edge_case_timestamps() {
        for ts in [
            "2024-01-15T10:30:00+00:00",
            "2024-01-15T10:30:00.123456789-05:30",
            "2024-06-30T23:59:59.000000001+14:00",
            "2024-12-31T23:59:59Z",
        ] {
            let created_at = DateTime::parse_from_rfc3339(ts).unwrap().with_timezone(&Utc);
            let cursor = Cursor { created_at, id: Uuid::new_v4() };
            let decoded = Cursor::decode(&cursor.encode()).expect("round trip");
            assert_eq!(decoded.created_at, cursor.created_at);
            assert_eq!(decoded.id, cursor.id);
        }
    }

    #[test]
    fn base64url_matches_rfc4648_vectors() {
        for (plain, encoded) in [("", ""), ("f", "Zg"), ("fo", "Zm8"), ("foo", "Zm9v"), ("foob", "Zm9vYg"), ("fooba", "Zm9vYmE"), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64url_encode(plain.as_bytes()), encoded);
            assert_eq!(base64url_decode(encoded).unwrap(), plain.as_bytes());
        }
        assert_eq!(base64url_encode(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn rejects_malformed_cursors() {
        assert!(Cursor::decode("not base64!").is_none());
        assert!(Cursor::decode("A").is_none());
        assert!(Cursor::decode(&base64url_encode(b"no-separator")).is_none());
        assert!(Cursor::decode(&base64url_encode(b"yesterday_not-a-uuid")).is_none());
    }
}
