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
