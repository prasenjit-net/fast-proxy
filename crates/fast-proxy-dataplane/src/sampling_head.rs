//! What the sampler keeps of a request's head and of a response's head, taken
//! before anything is known about whether the request will be sampled (that is
//! decided after the response). So it must be cheap: a few header values cloned
//! into fixed arrays (a clone of a header value shares its bytes) and one scan
//! of the request's headers for the things that make them unusual. Nothing is
//! allocated for a request with no extra headers configured.
//!
//! Spike G2 measured the clone of the allow-listed request headers at about
//! 77 ns per request.

use fast_proxy_sampling::UnusualFlags;
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Method, Uri};

/// More request headers than this is unusual.
const MAX_HEADER_COUNT: usize = 64;
/// A header value longer than this is unusual.
const MAX_VALUE_BYTES: usize = 8 * 1024;
/// A path and query longer than this is unusual.
const MAX_PATH_BYTES: usize = 8 * 1024;

const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The request headers every sample keeps (decision J15); the operator can add
/// more in the settings.
const REQUEST_COUNT: usize = 10;
const RESPONSE_COUNT: usize = 8;

const REQUEST_HEADERS: [HeaderName; REQUEST_COUNT] = [
    header::HOST,
    header::USER_AGENT,
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
    header::ACCEPT,
    header::ACCEPT_ENCODING,
    header::REFERER,
    X_FORWARDED_FOR,
    X_REQUEST_ID,
    header::ORIGIN,
];

/// The response headers every sample keeps.
const RESPONSE_HEADERS: [HeaderName; RESPONSE_COUNT] = [
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
    header::CONTENT_ENCODING,
    header::CACHE_CONTROL,
    header::LOCATION,
    header::RETRY_AFTER,
    header::SERVER,
    X_REQUEST_ID,
];

fn pick<const N: usize>(headers: &HeaderMap, names: &[HeaderName; N]) -> [Option<HeaderValue>; N] {
    std::array::from_fn(|index| headers.get(&names[index]).cloned())
}

fn text_of<const N: usize>(
    picked: [Option<HeaderValue>; N],
    names: &[HeaderName; N],
) -> Vec<(String, String)> {
    picked
        .into_iter()
        .zip(names)
        .filter_map(|(value, name)| {
            value.map(|value| {
                (
                    name.as_str().to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
        })
        .collect()
}

/// The selected request headers and what is unusual about the request's head.
pub(crate) struct HeadCapture {
    pub(crate) flags: UnusualFlags,
    picked: [Option<HeaderValue>; REQUEST_COUNT],
    extras: Vec<(String, HeaderValue)>,
}

impl HeadCapture {
    /// Takes the head of a request. `extra_names` are the operator's added
    /// headers (usually none).
    pub(crate) fn take(
        headers: &HeaderMap,
        method: &Method,
        uri: &Uri,
        extra_names: &[String],
    ) -> Self {
        let extras = extra_names
            .iter()
            .filter_map(|name| {
                headers
                    .get(name.as_str())
                    .map(|value| (name.to_ascii_lowercase(), value.clone()))
            })
            .collect();
        Self {
            flags: unusual_flags(headers, method, uri),
            picked: pick(headers, &REQUEST_HEADERS),
            extras,
        }
    }

    /// The headers as text, as received (redaction comes later).
    pub(crate) fn into_headers(self) -> Vec<(String, String)> {
        let mut headers = text_of(self.picked, &REQUEST_HEADERS);
        headers.extend(
            self.extras.into_iter().map(|(name, value)| {
                (name, String::from_utf8_lossy(value.as_bytes()).into_owned())
            }),
        );
        headers
    }
}

/// The selected response headers.
pub(crate) struct ResponseCapture {
    picked: [Option<HeaderValue>; RESPONSE_COUNT],
}

impl ResponseCapture {
    pub(crate) fn take(headers: &HeaderMap) -> Self {
        Self {
            picked: pick(headers, &RESPONSE_HEADERS),
        }
    }

    pub(crate) fn into_headers(self) -> Vec<(String, String)> {
        text_of(self.picked, &RESPONSE_HEADERS)
    }
}

/// What is unusual about a request's head (decision J7). The limits are
/// constants that only ever cause a request to be sampled, never refused.
pub(crate) fn unusual_flags(headers: &HeaderMap, method: &Method, uri: &Uri) -> UnusualFlags {
    let mut flags = UnusualFlags::default();
    if headers.len() > MAX_HEADER_COUNT {
        flags = flags.with(UnusualFlags::TOO_MANY_HEADERS);
    }
    for value in headers.values() {
        if value.len() > MAX_VALUE_BYTES {
            flags = flags.with(UnusualFlags::HUGE_HEADER_VALUE);
        }
        if value.to_str().is_err() {
            flags = flags.with(UnusualFlags::NON_VISIBLE_VALUE);
        }
    }
    let repeated = |name: &HeaderName| headers.get_all(name).iter().nth(1).is_some();
    if repeated(&header::HOST)
        || repeated(&header::CONTENT_LENGTH)
        || repeated(&header::CONTENT_TYPE)
    {
        flags = flags.with(UnusualFlags::DUPLICATE_SINGLETON);
    }
    if headers.contains_key(header::CONTENT_LENGTH)
        && headers.contains_key(header::TRANSFER_ENCODING)
    {
        flags = flags.with(UnusualFlags::LENGTH_WITH_TRANSFER_ENCODING);
    }
    if !is_standard(method) {
        flags = flags.with(UnusualFlags::UNKNOWN_METHOD);
    }
    let path_len = uri.path_and_query().map_or(0, |pq| pq.as_str().len());
    if path_len > MAX_PATH_BYTES {
        flags = flags.with(UnusualFlags::HUGE_PATH);
    }
    flags
}

fn is_standard(method: &Method) -> bool {
    [
        Method::GET,
        Method::HEAD,
        Method::POST,
        Method::PUT,
        Method::DELETE,
        Method::CONNECT,
        Method::OPTIONS,
        Method::TRACE,
        Method::PATCH,
    ]
    .contains(method)
}

/// The content type of a body worth capturing (decision J9): text, JSON, XML or
/// a form, sent as it is (no compression). `None` for anything else, including
/// a message with no content type.
pub(crate) fn capturable_content_type(headers: &HeaderMap) -> Option<String> {
    let content_type = headers.get(header::CONTENT_TYPE)?.to_str().ok()?;
    let media = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let text_like = media.starts_with("text/")
        || media == "application/json"
        || media.ends_with("+json")
        || media == "application/xml"
        || media.ends_with("+xml")
        || media == "application/x-www-form-urlencoded";
    let plain = headers
        .get(header::CONTENT_ENCODING)
        .is_none_or(|encoding| encoding.as_bytes().eq_ignore_ascii_case(b"identity"));
    (text_like && plain).then(|| content_type.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(text: &str) -> Uri {
        text.parse().unwrap()
    }

    fn flags(headers: &HeaderMap) -> UnusualFlags {
        unusual_flags(headers, &Method::GET, &uri("/"))
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn an_ordinary_request_has_no_flags() {
        let map = headers(&[
            ("host", "example.com"),
            ("accept", "*/*"),
            ("content-length", "12"),
        ]);
        assert!(flags(&map).is_empty());
    }

    #[test]
    fn too_many_headers_is_flagged_at_the_limit_plus_one() {
        let at_limit: Vec<(String, String)> = (0..MAX_HEADER_COUNT)
            .map(|n| (format!("x-h{n}"), "v".into()))
            .collect();
        let mut map = HeaderMap::new();
        for (name, value) in &at_limit {
            map.append(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        assert!(flags(&map).is_empty());
        map.append("x-one-more", HeaderValue::from_static("v"));
        assert!(flags(&map).contains(UnusualFlags::TOO_MANY_HEADERS));
    }

    #[test]
    fn a_huge_value_is_flagged_just_over_the_limit() {
        let mut map = HeaderMap::new();
        map.insert(
            "x-big",
            HeaderValue::from_str(&"a".repeat(MAX_VALUE_BYTES)).unwrap(),
        );
        assert!(flags(&map).is_empty());
        map.insert(
            "x-big",
            HeaderValue::from_str(&"a".repeat(MAX_VALUE_BYTES + 1)).unwrap(),
        );
        assert!(flags(&map).contains(UnusualFlags::HUGE_HEADER_VALUE));
    }

    #[test]
    fn a_value_that_is_not_visible_ascii_is_flagged() {
        let mut map = HeaderMap::new();
        map.insert(
            "x-bin",
            HeaderValue::from_bytes(&[b'a', 0xE9, b'b']).unwrap(),
        );
        assert!(flags(&map).contains(UnusualFlags::NON_VISIBLE_VALUE));
        let mut tab = HeaderMap::new();
        tab.insert("x-tab", HeaderValue::from_bytes(b"a\tb").unwrap());
        assert!(flags(&tab).is_empty(), "a tab is allowed in a header value");
    }

    #[test]
    fn a_repeated_singleton_header_is_flagged() {
        for name in ["host", "content-length", "content-type"] {
            let map = headers(&[(name, "1"), (name, "2")]);
            assert!(
                flags(&map).contains(UnusualFlags::DUPLICATE_SINGLETON),
                "{name}"
            );
        }
        let accepts = headers(&[("accept", "a"), ("accept", "b")]);
        assert!(flags(&accepts).is_empty(), "other headers may repeat");
    }

    #[test]
    fn a_length_together_with_a_transfer_encoding_is_flagged() {
        let map = headers(&[("content-length", "3"), ("transfer-encoding", "chunked")]);
        assert!(flags(&map).contains(UnusualFlags::LENGTH_WITH_TRANSFER_ENCODING));
    }

    #[test]
    fn an_unknown_method_and_a_huge_path_are_flagged() {
        let map = HeaderMap::new();
        for method in [Method::GET, Method::PATCH, Method::OPTIONS, Method::TRACE] {
            assert!(
                unusual_flags(&map, &method, &uri("/")).is_empty(),
                "{method}"
            );
        }
        let odd = Method::from_bytes(b"BREW").unwrap();
        assert!(unusual_flags(&map, &odd, &uri("/")).contains(UnusualFlags::UNKNOWN_METHOD));
        let long = format!("/{}", "a".repeat(MAX_PATH_BYTES));
        assert!(unusual_flags(&map, &Method::GET, &uri(&long)).contains(UnusualFlags::HUGE_PATH));
        let at_limit = format!("/{}", "a".repeat(MAX_PATH_BYTES - 1));
        assert!(unusual_flags(&map, &Method::GET, &uri(&at_limit)).is_empty());
    }

    #[test]
    fn the_selected_request_headers_are_kept_in_order_and_the_others_are_not() {
        let map = headers(&[
            ("host", "example.com"),
            ("authorization", "Bearer secret"),
            ("user-agent", "curl"),
            ("x-forwarded-for", "203.0.113.9"),
            ("x-private", "nope"),
        ]);
        let capture = HeadCapture::take(&map, &Method::GET, &uri("/"), &[]);
        assert_eq!(
            capture.into_headers(),
            [
                ("host".to_string(), "example.com".to_string()),
                ("user-agent".to_string(), "curl".to_string()),
                ("x-forwarded-for".to_string(), "203.0.113.9".to_string()),
            ],
            "the authorization header is not even held"
        );
    }

    #[test]
    fn extra_headers_are_added_by_name_case_insensitively_when_present() {
        let map = headers(&[("host", "h"), ("x-tenant", "acme")]);
        let capture = HeadCapture::take(
            &map,
            &Method::GET,
            &uri("/"),
            &["X-Tenant".to_string(), "x-absent".to_string()],
        );
        let kept = capture.into_headers();
        assert!(kept.contains(&("x-tenant".to_string(), "acme".to_string())));
        assert!(!kept.iter().any(|(name, _)| name == "x-absent"));
    }

    #[test]
    fn the_selected_response_headers_are_kept() {
        let map = headers(&[
            ("content-type", "text/plain"),
            ("set-cookie", "s=1"),
            ("retry-after", "30"),
            ("x-request-id", "r1"),
        ]);
        assert_eq!(
            ResponseCapture::take(&map).into_headers(),
            [
                ("content-type".to_string(), "text/plain".to_string()),
                ("retry-after".to_string(), "30".to_string()),
                ("x-request-id".to_string(), "r1".to_string()),
            ],
            "a cookie is not held"
        );
    }

    #[test]
    fn only_text_json_xml_and_form_bodies_that_are_not_compressed_are_capturable() {
        let allowed = [
            "text/plain",
            "text/html; charset=utf-8",
            "APPLICATION/JSON",
            "application/vnd.api+json",
            "application/xml",
            "application/atom+xml",
            "application/x-www-form-urlencoded",
        ];
        for content_type in allowed {
            let map = headers(&[("content-type", content_type)]);
            assert_eq!(capturable_content_type(&map).as_deref(), Some(content_type));
        }
        for content_type in [
            "image/png",
            "application/octet-stream",
            "application/pdf",
            "multipart/form-data",
        ] {
            let map = headers(&[("content-type", content_type)]);
            assert_eq!(capturable_content_type(&map), None, "{content_type}");
        }
        assert_eq!(
            capturable_content_type(&HeaderMap::new()),
            None,
            "no content type"
        );
    }

    #[test]
    fn a_compressed_body_is_not_capturable_but_identity_is() {
        let gzip = headers(&[
            ("content-type", "application/json"),
            ("content-encoding", "gzip"),
        ]);
        assert_eq!(capturable_content_type(&gzip), None);
        let identity = headers(&[
            ("content-type", "application/json"),
            ("content-encoding", "Identity"),
        ]);
        assert!(capturable_content_type(&identity).is_some());
    }
}
