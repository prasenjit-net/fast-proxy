//! The size checks a request passes before anything reaches an upstream
//! (ADR 0029): the request target's length, and a declared body length against
//! the route's cap. A body that streams past the cap is cut by the forwarder.

use http::{header, HeaderMap, Uri};

use crate::{FixedResponse, RequestLimits};

/// The body cap for a request on a route with `route_cap`, or the refusal it
/// gets before it is forwarded: `414` for a long target, `413` for a declared
/// `Content-Length` over the cap.
pub(crate) fn check(
    uri: &Uri,
    headers: &HeaderMap,
    limits: RequestLimits,
    route_cap: Option<u64>,
) -> Result<Option<u64>, FixedResponse> {
    let target = uri
        .path_and_query()
        .map_or(0, |target| target.as_str().len());
    if target > limits.max_uri_bytes {
        return Err(FixedResponse::UriTooLong);
    }
    let cap = route_cap.or(limits.max_body_bytes);
    let Some(cap) = cap else {
        return Ok(None);
    };
    if declared_length(headers).is_some_and(|length| length > cap) {
        return Err(FixedResponse::PayloadTooLarge);
    }
    Ok(Some(cap))
}

/// The `Content-Length` the client declared; hyper has already refused one
/// that is malformed or contradicts another.
fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn limits(max_body_bytes: Option<u64>) -> RequestLimits {
        RequestLimits {
            max_uri_bytes: 32,
            max_body_bytes,
        }
    }

    fn with_length(length: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_LENGTH,
            HeaderValue::from_str(length).unwrap(),
        );
        headers
    }

    #[test]
    fn a_long_target_is_refused_whatever_the_body() {
        let long: Uri = format!("/{}", "a".repeat(32)).parse().unwrap();
        assert_eq!(
            check(&long, &HeaderMap::new(), limits(None), None),
            Err(FixedResponse::UriTooLong)
        );
        let query: Uri = format!("/?q={}", "a".repeat(29)).parse().unwrap();
        assert_eq!(
            check(&query, &HeaderMap::new(), limits(None), None),
            Err(FixedResponse::UriTooLong),
            "the query counts"
        );
        let fits: Uri = format!("/{}", "a".repeat(31)).parse().unwrap();
        assert_eq!(
            check(&fits, &HeaderMap::new(), limits(None), None),
            Ok(None)
        );
    }

    #[test]
    fn the_route_cap_wins_over_the_default() {
        let uri: Uri = "/upload".parse().unwrap();
        let headers = with_length("1000");
        assert_eq!(
            check(&uri, &headers, limits(Some(10)), Some(2000)),
            Ok(Some(2000))
        );
        assert_eq!(
            check(&uri, &headers, limits(Some(2000)), Some(10)),
            Err(FixedResponse::PayloadTooLarge)
        );
        assert_eq!(
            check(&uri, &headers, limits(Some(1000)), None),
            Ok(Some(1000)),
            "exactly the cap"
        );
        assert_eq!(
            check(&uri, &headers, limits(Some(999)), None),
            Err(FixedResponse::PayloadTooLarge)
        );
    }

    #[test]
    fn without_a_declared_length_the_cap_is_left_to_the_stream() {
        let uri: Uri = "/upload".parse().unwrap();
        assert_eq!(
            check(&uri, &HeaderMap::new(), limits(Some(5)), None),
            Ok(Some(5))
        );
        assert_eq!(check(&uri, &HeaderMap::new(), limits(None), None), Ok(None));
    }
}
