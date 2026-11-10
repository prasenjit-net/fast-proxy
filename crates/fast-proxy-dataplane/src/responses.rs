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
}

impl FixedResponse {
    pub fn status(self) -> StatusCode {
        match self {
            Self::UnknownHost | Self::NoRoute => StatusCode::NOT_FOUND,
            Self::SubsystemDisabled | Self::NoHealthyUpstream => StatusCode::SERVICE_UNAVAILABLE,
            Self::MisdirectedRequest => StatusCode::MISDIRECTED_REQUEST,
            Self::BadGateway => StatusCode::BAD_GATEWAY,
            Self::GatewayTimeout => StatusCode::GATEWAY_TIMEOUT,
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
        }
    }

    /// Builds the response: JSON when `wants_json`, plain text otherwise,
    /// always carrying the request id.
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

    const ALL: [FixedResponse; 7] = [
        FixedResponse::UnknownHost,
        FixedResponse::SubsystemDisabled,
        FixedResponse::NoRoute,
        FixedResponse::NoHealthyUpstream,
        FixedResponse::MisdirectedRequest,
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
        assert_eq!(statuses, [404, 503, 404, 503, 421, 502, 504]);
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
}
