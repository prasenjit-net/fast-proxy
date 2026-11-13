use fast_proxy_model::SamplingSettings;

use super::*;

/// Appears in every secret below; the markers and the field names contain
/// neither letter pair, so no output may contain it.
const LEAK: &str = "ZQ";

fn redactor() -> Redactor {
    Redactor::default()
}

fn body(content_type: &str, text: &str) -> String {
    redactor().body(content_type, text.as_bytes(), false).text
}

/// Cuts `text` at every byte boundary and asserts no cut leaks a secret.
fn assert_no_prefix_leaks(redactor: &Redactor, content_type: &str, text: &str) {
    for cut in 0..=text.len() {
        if !text.is_char_boundary(cut) {
            continue;
        }
        let truncated = cut < text.len();
        let out = redactor.body(content_type, &text.as_bytes()[..cut], truncated);
        assert!(
            !out.text.contains(LEAK),
            "{content_type}: cut at {cut} of {text:?} leaked: {:?}",
            out.text
        );
    }
}

#[test]
fn headers_that_carry_credentials_are_redacted_whatever_their_case() {
    let redactor = redactor();
    for name in [
        "Authorization",
        "AUTHORIZATION",
        "proxy-authorization",
        "Cookie",
        "Set-Cookie",
        "X-Api-Key",
        "x-auth-token",
        "X-CSRF-Token",
        "X-Amz-Security-Token",
        "X-Session-Token",
        "X-Client-Secret",
        "X-Password",
    ] {
        assert_eq!(redactor.header_value(name, "ZQsecret"), REDACTED, "{name}");
    }
    for name in [
        "Host",
        "User-Agent",
        "Content-Type",
        "X-Request-Id",
        "Accept",
    ] {
        assert_eq!(redactor.header_value(name, "plain"), "plain", "{name}");
    }
}

#[test]
fn the_operators_additions_and_the_bypass_header_are_redacted_too() {
    let settings = SamplingSettings {
        redact_headers: vec!["X-Internal-Key".into()],
        ..SamplingSettings::default()
    };
    let redactor = Redactor::new(&settings, &["X-Maintenance-Pass".into()]);
    assert_eq!(redactor.header_value("x-internal-key", "v"), REDACTED);
    assert_eq!(redactor.header_value("X-MAINTENANCE-PASS", "v"), REDACTED);
    assert_eq!(redactor.header_value("accept", "v"), "v");
}

#[test]
fn additions_never_remove_a_built_in_name() {
    let settings = SamplingSettings {
        redact_headers: vec!["x-only-this".into()],
        redact_query_params: vec!["only".into()],
        redact_body_fields: vec!["only".into()],
        ..SamplingSettings::default()
    };
    let redactor = Redactor::new(&settings, &[]);
    assert_eq!(redactor.header_value("authorization", "v"), REDACTED);
    assert_eq!(
        redactor.query("password=ZQ1&only=x&a=b"),
        "password=[redacted]&only=[redacted]&a=b"
    );
    assert!(redactor
        .body("application/json", br#"{"password":"ZQ"}"#, false)
        .text
        .contains(REDACTED));
}

#[test]
fn sensitive_query_parameters_are_blanked_and_the_rest_is_kept() {
    let redactor = redactor();
    assert_eq!(
        redactor.query("page=2&token=ZQabc&sort=asc&API_KEY=ZQdef&code=ZQ9"),
        "page=2&token=[redacted]&sort=asc&API_KEY=[redacted]&code=[redacted]"
    );
    assert_eq!(
        redactor.query("access_token=ZQ&x"),
        "access_token=[redacted]&x"
    );
    assert_eq!(
        redactor.query("user_password=ZQ"),
        "user_password=[redacted]",
        "a word inside the name"
    );
    assert_eq!(
        redactor.query("pass%77ord=ZQ"),
        "pass%77ord=[redacted]",
        "an escaped name"
    );
    assert_eq!(redactor.query(""), "");
    assert_eq!(redactor.query("flag&plain=1"), "flag&plain=1");
    assert_eq!(
        redactor.query("keyboard=ok&monkey=ok"),
        "keyboard=ok&monkey=ok",
        "short names match exactly only"
    );
}

#[test]
fn json_members_with_sensitive_names_are_blanked() {
    let out = body(
        "application/json",
        r#"{"user":"ann","password":"ZQ1","nested":{"token":"ZQ2","ok":1},"list":[{"secret":"ZQ3"}],"card":4111111111111111,"n":null}"#,
    );
    assert_eq!(
        out,
        r#"{"user":"ann","password":"[redacted]","nested":{"token":"[redacted]","ok":1},"list":[{"secret":"[redacted]"}],"card":"[redacted]","n":null}"#
    );
}

#[test]
fn a_sensitive_member_holding_an_object_or_array_is_blanked_whole() {
    let out = body(
        "application/json",
        r#"{"secret":{"a":"ZQ","b":["ZQ",{"c":"}"}]},"after":"kept","token":["ZQ"]}"#,
    );
    assert_eq!(
        out,
        r#"{"secret":"[redacted]","after":"kept","token":"[redacted]"}"#
    );
}

#[test]
fn json_whitespace_escapes_and_case_are_handled() {
    let out = body(
        "application/json; charset=utf-8",
        "{ \"Password\" :\n  \"ZQ\\\"quoted\\\\\" , \"pass\\u0077ord\":\"ZQ2\", \"note\": \"a \\\"password\\\": \\\"x\\\"\" }",
    );
    assert!(!out.contains(LEAK), "{out}");
    assert!(
        out.contains(r#""note": "a \"password\": \"x\"""#),
        "text inside a string is not a member: {out}"
    );
}

#[test]
fn json_content_types_are_recognised_and_other_types_are_omitted() {
    let r = redactor();
    for ct in [
        "application/json",
        "APPLICATION/JSON",
        "application/vnd.api+json",
        "application/problem+json; charset=utf-8",
    ] {
        let out = r.body(ct, br#"{"password":"ZQ"}"#, false);
        assert_eq!(out.text, r#"{"password":"[redacted]"}"#, "{ct}");
        assert_eq!(out.omitted, None);
    }
    for ct in [
        "image/png",
        "application/octet-stream",
        "application/pdf",
        "",
    ] {
        let out = r.body(ct, b"ZQ binary", false);
        assert_eq!(out.omitted, Some("content_type"), "{ct}");
        assert!(out.text.is_empty());
    }
}

#[test]
fn form_fields_are_blanked_with_percent_and_plus_decoding() {
    let out = body(
        "application/x-www-form-urlencoded",
        "user=ann&pass%77ord=ZQ1&client%5Fsecret=ZQ2&q=a+b&Token=ZQ3&card=ZQ4",
    );
    assert_eq!(
        out,
        "user=ann&pass%77ord=[redacted]&client%5Fsecret=[redacted]&q=a+b&Token=[redacted]&card=[redacted]"
    );
}

#[test]
fn a_form_cut_before_its_equals_sign_drops_the_unsafe_tail() {
    let r = redactor();
    let out = r.body("application/x-www-form-urlencoded", b"user=ann&passw", true);
    assert_eq!(out.text, "user=ann");
    assert!(out.truncated);
    let whole = r.body("application/x-www-form-urlencoded", b"user=ann&flag", false);
    assert_eq!(
        whole.text, "user=ann&flag",
        "an uncut body keeps a bare field"
    );
    let cut_value = r.body(
        "application/x-www-form-urlencoded",
        b"user=ann&password=ZQ",
        true,
    );
    assert_eq!(cut_value.text, "user=ann&password=[redacted]");
}

#[test]
fn xml_and_plain_text_blank_the_value_after_a_sensitive_name() {
    let xml = body(
        "text/xml",
        "<login><user>ann</user><password>ZQ1</password><api_key>ZQ2</api_key></login>",
    );
    assert_eq!(xml, "<login><user>ann</user><password>[redacted]</password><api_key>[redacted]</api_key></login>");
    let text = body(
        "text/plain",
        "user=ann password=ZQ1, token: ZQ2; secret = 'ZQ 3' note=fine\nauthorization=ZQ4",
    );
    assert_eq!(text, "user=ann password=[redacted], token: [redacted]; secret = '[redacted]' note=fine\nauthorization=[redacted]");
    let words = body("text/plain", "access_token=ZQ5 and tokenizer=ok");
    assert_eq!(words, "access_token=[redacted] and tokenizer=ok");
}

#[test]
fn text_that_is_json_is_redacted_as_json() {
    let out = body("text/plain", r#"{"password":"ZQ"}"#);
    assert_eq!(out, r#"{"password":"[redacted]"}"#);
}

#[test]
fn a_body_with_nothing_sensitive_comes_back_unchanged() {
    let json = r#"{"a":1,"b":[true,null,"x"],"c":{"d":"é"}}"#;
    assert_eq!(body("application/json", json), json);
    assert_eq!(
        body("application/x-www-form-urlencoded", "a=1&b=2"),
        "a=1&b=2"
    );
    assert_eq!(body("text/plain", "hello world"), "hello world");
    assert_eq!(body("application/json", ""), "");
}

#[test]
fn multibyte_text_survives_and_a_cut_inside_a_character_is_safe() {
    let r = redactor();
    let text = r#"{"name":"héllo wörld","password":"ZQ"}"#;
    let bytes = text.as_bytes();
    for cut in 0..=bytes.len() {
        let out = r.body("application/json", &bytes[..cut], true);
        assert!(!out.text.contains(LEAK), "cut {cut}: {:?}", out.text);
    }
}

#[test]
fn no_cut_of_a_json_body_leaks_a_secret() {
    let r = redactor();
    for text in [
        r#"{"user":"ann","password":"ZQ-top-secret","n":1}"#,
        r#"{"a":{"b":{"secret":"ZQ-deep"}},"c":[1,2]}"#,
        r#"{"list":[{"token":"ZQ-1"},{"token":"ZQ-2"}],"x":"y"}"#,
        r#"{"note":"has \"quote\"","client_secret":"ZQ\"esc\\aped"}"#,
        "{ \"Password\" :\n \"ZQ spaced\" , \"api_key\":\n 12345ZQ }",
        r#"{"password":"ZQ-unicode-key"}"#,
        r#"{"secret":{"deep":["ZQ",{"x":"ZQ"}]},"k":1}"#,
        r#"[{"password":"ZQ"},{"password":"ZQ"}]"#,
    ] {
        assert_no_prefix_leaks(&r, "application/json", text);
    }
}

#[test]
fn no_cut_of_a_form_xml_or_text_body_leaks_a_secret() {
    let r = redactor();
    for text in [
        "user=ann&password=ZQ-form&x=1",
        "a=1&client_secret=ZQ%20encoded&b=2",
        "pass%77ord=ZQ-escaped-name",
    ] {
        assert_no_prefix_leaks(&r, "application/x-www-form-urlencoded", text);
    }
    for text in [
        "<a><password>ZQ-xml</password><b>1</b></a>",
        "<api_key>ZQ</api_key>",
    ] {
        assert_no_prefix_leaks(&r, "application/xml", text);
    }
    for text in [
        "user=ann password=ZQ-text note=ok",
        "token: ZQ-colon; secret = 'ZQ quoted'",
        "access_token=ZQ",
    ] {
        assert_no_prefix_leaks(&r, "text/plain", text);
    }
}

#[test]
fn random_bodies_never_leak_at_any_cut() {
    let r = redactor();
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move |bound: usize| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) as usize) % bound
    };
    let keys = [
        "id",
        "name",
        "password",
        "Token",
        "note",
        "secret",
        "client_secret",
        "a b",
        "q\\\"x",
    ];
    for _ in 0..200 {
        let mut text = String::from("{");
        for n in 0..(1 + next(6)) {
            if n > 0 {
                text.push(',');
            }
            let key = keys[next(keys.len())];
            let secret_value = key.to_ascii_lowercase().contains("pass")
                || key.to_ascii_lowercase().contains("secret")
                || key.to_ascii_lowercase().contains("token");
            let value = match (secret_value, next(4)) {
                (true, 0) => "\"ZQ-a-secret\"".to_string(),
                (true, 1) => "12345ZQ".to_string(),
                (true, 2) => "{\"x\":[\"ZQ\",{\"y\":\"ZQ\"}]}".to_string(),
                (true, _) => "[\"ZQ\",\"ZQ\"]".to_string(),
                (false, 0) => "\"plain\"".to_string(),
                (false, 1) => "42".to_string(),
                (false, 2) => "{\"inner\":\"fine\"}".to_string(),
                (false, _) => "[1,2,3]".to_string(),
            };
            text.push_str(&format!("\"{key}\":{value}"));
        }
        text.push('}');
        assert_no_prefix_leaks(&r, "application/json", &text);
    }
}

#[test]
fn the_truncated_flag_follows_the_cut_and_a_dropped_tail() {
    let r = redactor();
    assert!(!r.body("application/json", br#"{"a":1}"#, false).truncated);
    assert!(r.body("application/json", br#"{"a":1,"#, true).truncated);
    let dropped = r.body("application/json", br#"{"a":1,"pass"#, true);
    assert_eq!(dropped.text, r#"{"a":1,"#);
    assert!(dropped.truncated);
    assert!(r.body("text/plain", b"hello passw", true).truncated);
    assert_eq!(r.body("text/plain", b"hello passw", true).text, "hello ");
    assert_eq!(
        r.body("text/plain", b"hello world", true).text,
        "hello world",
        "a cut that cannot start a name is kept"
    );
}
