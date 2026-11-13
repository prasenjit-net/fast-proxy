//! Redaction (ADR 0021): header values, query parameters and bodies are
//! blanked by name before a sample is stored anywhere. The built-in lists
//! cannot be shortened, only added to. Bodies are cut at a size cap, so the
//! redactor works on a prefix: it is lexical, never a parse followed by a
//! serialization, and anything it cannot judge at the cut is dropped.
//!
//! Residual risk, stated plainly in the docs: a secret in a field nobody
//! listed, in a path segment, or in free text is not found.

use std::collections::HashSet;

use fast_proxy_model::SamplingSettings;

/// What a redacted value becomes.
pub const REDACTED: &str = "[redacted]";

/// Header names always redacted (lower case).
pub const BUILT_IN_HEADERS: [&str; 8] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-auth-token",
    "x-csrf-token",
    "x-amz-security-token",
];

/// Query parameter names always redacted.
pub const BUILT_IN_QUERY_PARAMS: [&str; 15] = [
    "token",
    "access_token",
    "refresh_token",
    "id_token",
    "key",
    "api_key",
    "apikey",
    "secret",
    "password",
    "passwd",
    "signature",
    "sig",
    "code",
    "session",
    "ticket",
];

/// Body field names always redacted.
pub const BUILT_IN_BODY_FIELDS: [&str; 12] = [
    "password",
    "passwd",
    "secret",
    "token",
    "access_token",
    "refresh_token",
    "client_secret",
    "api_key",
    "authorization",
    "card",
    "cvv",
    "ssn",
];

/// A name that contains one of these is redacted whatever else it says
/// (`user_password`, `X-Session-Token`, `clientSecret`).
const SENSITIVE_WORDS: [&str; 4] = ["password", "passwd", "secret", "token"];

/// A body after redaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedBody {
    pub text: String,
    /// The body was cut at the cap, or its unjudgeable tail was dropped.
    pub truncated: bool,
    /// Why there is no text, when there is none.
    pub omitted: Option<&'static str>,
}

/// The built-in lists plus the operator's additions, ready to apply.
#[derive(Debug, Clone)]
pub struct Redactor {
    headers: HashSet<String>,
    query_params: HashSet<String>,
    body_fields: HashSet<String>,
}

impl Default for Redactor {
    /// Only the built-in lists.
    fn default() -> Self {
        Self::new(&SamplingSettings::default(), &[])
    }
}

impl Redactor {
    /// Builds a redactor from the settings' additions and `extra_headers`
    /// (the bypass header names of the maintenance scopes).
    pub fn new(settings: &SamplingSettings, extra_headers: &[String]) -> Self {
        let set = |built_in: &[&str], additions: &[String]| -> HashSet<String> {
            built_in
                .iter()
                .map(|name| (*name).to_string())
                .chain(additions.iter().map(|name| name.to_ascii_lowercase()))
                .collect()
        };
        let mut headers = set(&BUILT_IN_HEADERS, &settings.redact_headers);
        headers.extend(extra_headers.iter().map(|name| name.to_ascii_lowercase()));
        Self {
            headers,
            query_params: set(&BUILT_IN_QUERY_PARAMS, &settings.redact_query_params),
            body_fields: set(&BUILT_IN_BODY_FIELDS, &settings.redact_body_fields),
        }
    }

    fn matches(set: &HashSet<String>, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        set.contains(&lower) || SENSITIVE_WORDS.iter().any(|word| lower.contains(word))
    }

    /// Whether a header's value must not be stored.
    pub fn is_sensitive_header(&self, name: &str) -> bool {
        Self::matches(&self.headers, name)
    }

    /// The value to store for a header: the value itself, or [`REDACTED`].
    pub fn header_value(&self, name: &str, value: &str) -> String {
        if self.is_sensitive_header(name) {
            REDACTED.to_string()
        } else {
            value.to_string()
        }
    }

    /// The query string with the values of sensitive parameters blanked.
    pub fn query(&self, query: &str) -> String {
        query
            .split('&')
            .map(|part| match part.split_once('=') {
                Some((name, _)) if Self::matches(&self.query_params, &percent_decode(name)) => {
                    format!("{name}={REDACTED}")
                }
                _ => part.to_string(),
            })
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Redacts a captured body prefix. `truncated` says the body was longer
    /// than what was captured.
    pub fn body(&self, content_type: &str, bytes: &[u8], truncated: bool) -> RedactedBody {
        let text = String::from_utf8_lossy(bytes);
        let media = media_type(content_type);
        let kind = BodyKind::of(&media, &text);
        let Some(kind) = kind else {
            return RedactedBody {
                text: String::new(),
                truncated,
                omitted: Some("content_type"),
            };
        };
        let (text, dropped) = match kind {
            BodyKind::Json => self.redact_json(&text),
            BodyKind::Form => self.redact_form(&text, truncated),
            BodyKind::Loose => self.redact_loose(&text, truncated),
        };
        RedactedBody {
            text,
            truncated: truncated || dropped,
            omitted: None,
        }
    }

    fn is_body_field(&self, name: &str) -> bool {
        Self::matches(&self.body_fields, name)
    }

    /// JSON, lexically. Returns the text and whether a tail was dropped.
    fn redact_json(&self, text: &str) -> (String, bool) {
        let bytes = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut previous = b' ';
        let mut i = 0;
        while i < bytes.len() {
            let byte = bytes[i];
            if byte != b'"' {
                let ch = text[i..].chars().next().unwrap_or(' ');
                out.push(ch);
                if !ch.is_whitespace() {
                    previous = byte;
                }
                i += ch.len_utf8();
                continue;
            }
            let token_start = i;
            let Some(end) = string_end(bytes, i) else {
                // Cut inside a string. In a key position it may be the start of
                // a sensitive name, so it goes; as a value it is kept.
                if matches!(previous, b'{' | b',') {
                    return (out, true);
                }
                out.push_str(&text[i..]);
                return (out, false);
            };
            out.push_str(&text[i..end]);
            previous = b'"';
            i = end;
            let colon = skip_whitespace(bytes, i);
            let is_key = bytes.get(colon) == Some(&b':');
            if !is_key || !self.is_body_field(&decode_json_string(&text[token_start..end])) {
                continue;
            }
            out.push_str(&text[i..=colon]);
            let value_start = skip_whitespace(bytes, colon + 1);
            out.push_str(&text[colon + 1..value_start]);
            let value_end = value_end(bytes, value_start);
            if value_end > value_start {
                out.push('"');
                out.push_str(REDACTED);
                out.push('"');
            }
            previous = b':';
            i = value_end;
        }
        (out, false)
    }

    /// `a=b&c=d`. A last pair cut before its `=` could be the start of a
    /// sensitive name, so it is dropped.
    fn redact_form(&self, text: &str, truncated: bool) -> (String, bool) {
        let mut parts: Vec<&str> = text.split('&').collect();
        let mut dropped = false;
        if truncated && parts.last().is_some_and(|last| !last.contains('=')) {
            parts.pop();
            dropped = true;
        }
        let redacted: Vec<String> = parts
            .iter()
            .map(|part| match part.split_once('=') {
                Some((name, _)) if self.is_body_field(&percent_decode(name)) => {
                    format!("{name}={REDACTED}")
                }
                _ => (*part).to_string(),
            })
            .collect();
        (redacted.join("&"), dropped)
    }

    /// Text and XML: the value after a sensitive name and a `=`, `:` or `>`.
    fn redact_loose(&self, text: &str, truncated: bool) -> (String, bool) {
        let bytes = text.as_bytes();
        let lower = text.to_ascii_lowercase();
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let mut names: Vec<&str> = self.body_fields.iter().map(String::as_str).collect();
        names.extend(SENSITIVE_WORDS);
        for name in names {
            let mut from = 0;
            while let Some(found) = lower[from..].find(name) {
                let start = from + found;
                from = start + name.len();
                let core_word = SENSITIVE_WORDS.contains(&name);
                let boundary_before = start == 0 || !is_word_byte(bytes[start - 1]);
                if !core_word && !boundary_before {
                    continue;
                }
                if let Some(range) = value_after_name(bytes, from) {
                    ranges.push(range);
                }
            }
        }
        ranges.sort_unstable();
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0;
        for (start, end) in ranges {
            if start < cursor {
                continue;
            }
            out.push_str(&text[cursor..start]);
            out.push_str(REDACTED);
            cursor = end;
        }
        out.push_str(&text[cursor..]);
        if truncated {
            return self.drop_partial_name(out);
        }
        (out, false)
    }

    /// A cut can end inside a sensitive name (`...passw`); the part of it that
    /// is visible says nothing yet, but the next bytes would be a secret, so
    /// the partial name is dropped.
    fn drop_partial_name(&self, mut text: String) -> (String, bool) {
        let run_start = text
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .map_or(0, |at| at + 1);
        let run = text[run_start..].to_ascii_lowercase();
        let partial = run.len() >= 2
            && self
                .body_fields
                .iter()
                .map(String::as_str)
                .chain(SENSITIVE_WORDS)
                .any(|name| name.starts_with(&run) && name != run);
        if partial {
            text.truncate(run_start);
            return (text, true);
        }
        (text, false)
    }
}

enum BodyKind {
    Json,
    Form,
    Loose,
}

impl BodyKind {
    fn of(media: &str, text: &str) -> Option<Self> {
        if media == "application/json" || media.ends_with("+json") {
            return Some(Self::Json);
        }
        if media == "application/x-www-form-urlencoded" {
            return Some(Self::Form);
        }
        if media == "application/xml" || media.ends_with("+xml") || media.starts_with("text/") {
            let looks_like_json = media.starts_with("text/")
                && !media.contains("xml")
                && matches!(text.trim_start().as_bytes().first(), Some(b'{' | b'['));
            return Some(if looks_like_json {
                Self::Json
            } else {
                Self::Loose
            });
        }
        None
    }
}

fn media_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn skip_whitespace(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// The index just after the closing quote of the string that starts at
/// `start`, or `None` when the text ends first.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// The index just after the JSON value that starts at `start` (a string, a
/// nested object or array, or a bare scalar); the end of the text when it is
/// cut short.
fn value_end(bytes: &[u8], start: usize) -> usize {
    match bytes.get(start) {
        None => start,
        Some(b'"') => string_end(bytes, start).unwrap_or(bytes.len()),
        Some(b'{' | b'[') => {
            let mut depth = 0usize;
            let mut i = start;
            while i < bytes.len() {
                match bytes[i] {
                    b'"' => match string_end(bytes, i) {
                        Some(end) => {
                            i = end;
                            continue;
                        }
                        None => return bytes.len(),
                    },
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return i + 1;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            bytes.len()
        }
        Some(_) => {
            let mut i = start;
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && !matches!(bytes[i], b',' | b'}' | b']')
            {
                i += 1;
            }
            i
        }
    }
}

/// Decodes the escapes of a JSON string token (quotes included) well enough to
/// compare a key with a name.
fn decode_json_string(token: &str) -> String {
    let inner = token
        .strip_prefix('"')
        .map(|rest| rest.strip_suffix('"').unwrap_or(rest))
        .unwrap_or(token);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Some(decoded) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(decoded);
                }
            }
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// `%XX` and `+` in a form or query name.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(decoded) => {
                        out.push(decoded);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// After a sensitive name that ends at `from`: the range of the value that
/// follows a `=`, `:` or `>` (with optional quotes and spaces), if there is one.
fn value_after_name(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut i = from;
    if matches!(bytes.get(i), Some(b'"' | b'\'')) {
        i += 1;
    }
    i = skip_inline_spaces(bytes, i);
    if !matches!(bytes.get(i), Some(b'=' | b':' | b'>')) {
        return None;
    }
    i = skip_inline_spaces(bytes, i + 1);
    let quote = match bytes.get(i) {
        Some(q @ (b'"' | b'\'')) => {
            i += 1;
            Some(*q)
        }
        _ => None,
    };
    let start = i;
    while i < bytes.len() {
        let byte = bytes[i];
        let stop = match quote {
            Some(q) => byte == q,
            None => {
                byte.is_ascii_whitespace()
                    || matches!(byte, b'&' | b',' | b';' | b'<' | b'"' | b'\'' | b')' | b'}')
            }
        };
        if stop {
            break;
        }
        i += 1;
    }
    (i > start).then_some((start, i))
}

fn skip_inline_spaces(bytes: &[u8], mut at: usize) -> usize {
    while matches!(bytes.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    at
}
