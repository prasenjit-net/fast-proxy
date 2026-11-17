use bytes::Bytes;
use http::{header, HeaderMap, Response, StatusCode};
use http_body_util::{BodyExt, Full};

use crate::request_id::{header_value, REQUEST_ID};
use crate::ResponseBody;

/// The responses the proxy itself produces, as opposed to upstream ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedResponse {
    /// No subsystem answers to the request's host.
    UnknownHost,
    /// The subsystem exists but is switched off.
    SubsystemDisabled,
    /// The subsystem has no route for the path.
    NoRoute,
    /// The request reached a connection whose certificate was not meant for its
    /// host (or a host not served over HTTPS): the client should open another
    /// connection (RFC 9110, 421).
    MisdirectedRequest,
    /// The route's upstream group has no endpoint that may take the request.
    NoHealthyUpstream,
    /// The upstream refused or dropped the connection.
    BadGateway,
    /// The upstream did not answer in time.
    GatewayTimeout,
    /// The request body is larger than the route accepts (ADR 0029).
    PayloadTooLarge,
    /// The request target is longer than the proxy accepts (ADR 0029).
    UriTooLong,
    /// The request body stopped arriving (ADR 0029).
    RequestTimeout,
    /// The client is over its rate limit (ADR 0030); answered with `Retry-After`.
    TooManyRequests,
    /// Every endpoint that may take the request is at its request cap (ADR 0031).
    EndpointCapacity,
}

/// Marks a response as a refusal by one of the proxy's limits, with the reason:
/// the pipeline records it in the access log, the metrics and a sample, whichever
/// layer refused the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyRefusal(pub &'static str);

impl FixedResponse {
    pub fn status(self) -> StatusCode {
        match self {
            Self::UnknownHost | Self::NoRoute => StatusCode::NOT_FOUND,
            Self::SubsystemDisabled | Self::NoHealthyUpstream => StatusCode::SERVICE_UNAVAILABLE,
            Self::MisdirectedRequest => StatusCode::MISDIRECTED_REQUEST,
            Self::BadGateway => StatusCode::BAD_GATEWAY,
            Self::GatewayTimeout => StatusCode::GATEWAY_TIMEOUT,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UriTooLong => StatusCode::URI_TOO_LONG,
            Self::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Self::EndpointCapacity => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::UnknownHost => "unknown_host",
            Self::SubsystemDisabled => "subsystem_disabled",
            Self::NoRoute => "no_route",
            Self::NoHealthyUpstream => "no_healthy_upstream",
            Self::MisdirectedRequest => "misdirected_request",
            Self::BadGateway => "bad_gateway",
            Self::GatewayTimeout => "gateway_timeout",
            Self::PayloadTooLarge => "payload_too_large",
            Self::UriTooLong => "uri_too_long",
            Self::RequestTimeout => "request_timeout",
            Self::TooManyRequests => "rate_limited",
            Self::EndpointCapacity => "endpoint_capacity",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::UnknownHost => "unknown host",
            Self::SubsystemDisabled => "service unavailable",
            Self::NoRoute => "no route matches this path",
            Self::NoHealthyUpstream => "no healthy upstream",
            Self::MisdirectedRequest => "this host is not served on this connection",
            Self::BadGateway => "bad gateway",
            Self::GatewayTimeout => "gateway timeout",
            Self::PayloadTooLarge => "request body too large",
            Self::UriTooLong => "request target too long",
            Self::RequestTimeout => "request body timed out",
            Self::TooManyRequests => "too many requests",
            Self::EndpointCapacity => "upstream at capacity",
        }
    }

    /// The reason a limit refused the request, for the responses that are
    /// refusals; `None` for every other fixed response.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            Self::PayloadTooLarge
            | Self::UriTooLong
            | Self::RequestTimeout
            | Self::TooManyRequests
            | Self::EndpointCapacity => Some(self.code()),
            _ => None,
        }
    }

    /// Builds the response: JSON when `wants_json`, plain text otherwise,
    /// always carrying the request id. A refusal also carries its
    /// [`ProxyRefusal`].
    pub fn render(self, request_id: &str, wants_json: bool) -> Response<ResponseBody> {
        let (content_type, body) = if wants_json {
            let body = format!(
                r#"{{"error":{{"code":"{}","message":"{}","requestId":"{}"}}}}"#,
                self.code(),
                self.message(),
                json_escape(request_id)
            );
            ("application/json", body)
        } else {
            ("text/plain; charset=utf-8", format!("{}\n", self.message()))
        };
        let mut response = Response::new(boxed(body));
        *response.status_mut() = self.status();
        let headers = response.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static(content_type),
        );
        headers.insert(REQUEST_ID, header_value(request_id));
        if let Some(reason) = self.refusal() {
            response.extensions_mut().insert(ProxyRefusal(reason));
        }
        response
    }
}

/// Whether the client's `Accept` header asks for JSON.
pub fn accepts_json(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.to_ascii_lowercase().contains("application/json"))
}

/// How a client prefers a maintenance answer: JSON, else HTML when it accepts
/// that, else plain text.
pub fn response_format(headers: &HeaderMap) -> fast_proxy_routing::ResponseFormat {
    use fast_proxy_routing::ResponseFormat;
    if accepts_json(headers) {
        return ResponseFormat::Json;
    }
    let wants_html = headers
        .get_all(header::ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.to_ascii_lowercase().contains("text/html"));
    if wants_html {
        ResponseFormat::Html
    } else {
        ResponseFormat::Text
    }
}

/// Request ids are visible ASCII, so only `"` and `\` need escaping.
fn json_escape(raw: &str) -> String {
    raw.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(crate) fn boxed(body: String) -> ResponseBody {
    Full::new(Bytes::from(body))
        .map_err(|never| match never {})
        .boxed()
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;

    use super::*;

    const ALL: [FixedResponse; 12] = [
        FixedResponse::UnknownHost,
        FixedResponse::SubsystemDisabled,
        FixedResponse::NoRoute,
        FixedResponse::NoHealthyUpstream,
        FixedResponse::MisdirectedRequest,
        FixedResponse::PayloadTooLarge,
        FixedResponse::UriTooLong,
        FixedResponse::RequestTimeout,
        FixedResponse::TooManyRequests,
        FixedResponse::EndpointCapacity,
        FixedResponse::BadGateway,
        FixedResponse::GatewayTimeout,
    ];

    async fn body_text(response: Response<ResponseBody>) -> String {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn maps_each_kind_to_its_status() {
        let statuses: Vec<u16> = ALL.iter().map(|r| r.status().as_u16()).collect();
        assert_eq!(
            statuses,
            [404, 503, 404, 503, 421, 413, 414, 408, 429, 503, 502, 504]
        );
    }

    #[tokio::test]
    async fn renders_plain_text_by_default_with_the_request_id() {
        for kind in ALL {
            let response = kind.render("rid-1", false);
            assert_eq!(response.status(), kind.status());
            assert_eq!(response.headers()["x-request-id"], "rid-1");
            assert_eq!(
                response.headers()["content-type"],
                "text/plain; charset=utf-8"
            );
            assert_eq!(body_text(response).await, format!("{}\n", kind.message()));
        }
    }

    #[tokio::test]
    async fn renders_json_when_the_client_accepts_it() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            "text/html, Application/JSON;q=0.9".parse().unwrap(),
        );
        assert!(accepts_json(&headers));
        let response = FixedResponse::UnknownHost.render("rid-2", true);
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(
            body_text(response).await,
            r#"{"error":{"code":"unknown_host","message":"unknown host","requestId":"rid-2"}}"#
        );
    }

    #[tokio::test]
    async fn json_bodies_are_always_valid_even_for_hostile_ids() {
        let response = FixedResponse::NoRoute.render(r#"a"b\c"#, true);
        assert!(body_text(response)
            .await
            .contains(r#""requestId":"a\"b\\c""#));
    }

    #[test]
    fn only_refusals_are_marked_and_each_with_its_code() {
        let marked: Vec<(FixedResponse, &str)> = ALL
            .iter()
            .filter_map(|kind| {
                let response = kind.render("rid", false);
                response
                    .extensions()
                    .get::<ProxyRefusal>()
                    .map(|refusal| (*kind, refusal.0))
            })
            .collect();
        assert_eq!(
            marked,
            [
                (FixedResponse::PayloadTooLarge, "payload_too_large"),
                (FixedResponse::UriTooLong, "uri_too_long"),
                (FixedResponse::RequestTimeout, "request_timeout"),
                (FixedResponse::TooManyRequests, "rate_limited"),
                (FixedResponse::EndpointCapacity, "endpoint_capacity"),
            ]
        );
    }
}
