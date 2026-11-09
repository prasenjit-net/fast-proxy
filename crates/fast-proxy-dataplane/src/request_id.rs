use http::{HeaderMap, HeaderName, HeaderValue};
use rand::Rng;

/// The header carrying the request id in both directions.
pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

const MAX_LEN: usize = 128;

/// The id to use for a request: the client's own if it is a sane
/// visible-ASCII token, otherwise a freshly generated one.
pub fn request_id_for(headers: &HeaderMap) -> String {
    headers
        .get(&REQUEST_ID)
        .and_then(|value| value.to_str().ok())
        .filter(|id| is_acceptable(id))
        .map_or_else(generate, str::to_string)
}

fn is_acceptable(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_LEN && id.bytes().all(|b| b.is_ascii_graphic())
}

fn generate() -> String {
    format!("{:032x}", rand::thread_rng().gen::<u128>())
}

/// The id as a header value. Ids are visible ASCII by construction.
pub fn header_value(id: &str) -> HeaderValue {
    HeaderValue::from_str(id).unwrap_or_else(|_| HeaderValue::from_static("invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(value: &[u8]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(REQUEST_ID, HeaderValue::from_bytes(value).unwrap());
        headers
    }

    #[test]
    fn keeps_a_sane_client_id() {
        assert_eq!(request_id_for(&headers_with(b"abc-123")), "abc-123");
    }

    #[test]
    fn generates_when_absent() {
        let id = request_id_for(&HeaderMap::new());
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(id, request_id_for(&HeaderMap::new()));
    }

    #[test]
    fn replaces_unacceptable_client_ids() {
        for bad in [
            &b""[..],
            b"has space",
            b"tab\there",
            &[b'a'; 129],
            b"\xff\xfe",
        ] {
            let id = request_id_for(&headers_with(bad));
            assert_eq!(id.len(), 32, "{bad:?}");
        }
    }

    #[test]
    fn accepts_the_maximum_length() {
        let id = "a".repeat(128);
        assert_eq!(request_id_for(&headers_with(id.as_bytes())), id);
    }

    #[test]
    fn header_value_falls_back_instead_of_panicking() {
        assert_eq!(header_value("ok"), "ok");
        assert_eq!(header_value("bad\nvalue"), "invalid");
    }
}
