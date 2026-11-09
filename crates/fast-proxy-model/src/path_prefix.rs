use crate::FieldError;

/// Validates a path prefix and strips a trailing slash (except for `/` itself).
fn normalize(field: &str, raw: &str) -> Result<String, FieldError> {
    let invalid = |message: &str| Err(FieldError::new(field, message));
    if !raw.starts_with('/') {
        return invalid("must start with '/'");
    }
    if raw
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '?' | '#' | '\\'))
    {
        return invalid("must not contain whitespace, control characters, '?', '#', or '\\'");
    }
    if raw.contains("//") {
        return invalid("must not contain '//'");
    }
    let trimmed = if raw.len() > 1 {
        raw.trim_end_matches('/')
    } else {
        raw
    };
    if trimmed
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return invalid("must not contain '.' or '..' segments");
    }
    Ok(trimmed.to_string())
}

/// The path prefix a route matches on: `/` or `/seg[/seg]*`, no trailing slash.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoutePrefix(String);

impl RoutePrefix {
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        normalize("prefix", raw).map(Self)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A path prepended to the forwarded request path. Empty means nothing is
/// prepended; otherwise same shape as [`RoutePrefix`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct UpstreamPrefix(String);

impl UpstreamPrefix {
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        if raw.is_empty() {
            return Ok(Self::default());
        }
        let normalized = normalize("upstream_prefix", raw)?;
        Ok(Self(if normalized == "/" {
            String::new()
        } else {
            normalized
        }))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_prefix_normalizes_trailing_slashes() {
        for (raw, expected) in [
            ("/", "/"),
            ("/api", "/api"),
            ("/api/", "/api"),
            ("/api/v1", "/api/v1"),
            ("/a.b/c-d_e", "/a.b/c-d_e"),
            ("/a..b", "/a..b"),
        ] {
            assert_eq!(RoutePrefix::parse(raw).unwrap().as_str(), expected, "{raw}");
        }
    }

    #[test]
    fn route_prefix_rejects_unsafe_input_with_a_prefix_field_error() {
        for raw in [
            "", "api", "/a?x=1", "/a#frag", "//", "/a//b", "/a//", "/a/../b", "/..", "/./a",
            "/a/.", "/a b", "/a\tb", "/a\nb", "/a\\b", "/a\u{7f}",
        ] {
            let err = RoutePrefix::parse(raw).unwrap_err();
            assert_eq!(err.field, "prefix", "{raw:?}");
        }
    }

    #[test]
    fn upstream_prefix_may_be_empty_and_root_means_empty() {
        assert_eq!(UpstreamPrefix::parse("").unwrap().as_str(), "");
        assert_eq!(UpstreamPrefix::parse("/").unwrap().as_str(), "");
        assert_eq!(UpstreamPrefix::parse("/v2/").unwrap().as_str(), "/v2");
        assert_eq!(UpstreamPrefix::default().as_str(), "");
    }

    #[test]
    fn upstream_prefix_rejects_unsafe_input_with_its_own_field_name() {
        for raw in ["v2", "/a/../b", "/a?x", "/a//b"] {
            let err = UpstreamPrefix::parse(raw).unwrap_err();
            assert_eq!(err.field, "upstream_prefix", "{raw:?}");
        }
    }
}
