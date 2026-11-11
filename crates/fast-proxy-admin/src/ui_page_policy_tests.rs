//! The page the admin plane serves, checked against the policy it sends with
//! it: a browser enforcing `script-src 'self'` must be able to run the
//! built UI, and nothing in the page may reach for another origin.
//!
//! These read the embedded `ui/dist`, so they need `npm run build` first (CI
//! hands the built bundle to the Rust jobs).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;

use crate::test_support::TestApp;

async fn get(app: &TestApp, path: &str) -> axum::response::Response {
    app.send(
        Request::builder()
            .uri(path)
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// The value of `attribute` in a tag's attribute text, if it has one.
fn attribute<'a>(attributes: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("{name}=\"");
    let start = attributes.find(&marker)? + marker.len();
    let end = attributes[start..].find('"')? + start;
    Some(&attributes[start..end])
}

/// Each `<tag ...>` of `html` as (attributes, text up to the closing tag).
fn elements<'a>(html: &'a str, tag: &str) -> Vec<(&'a str, &'a str)> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find(&open) {
        let after = &rest[at + open.len()..];
        // `<link` must not match `<linkfoo`; a tag name ends at space, `>` or `/`.
        if !after.starts_with([' ', '>', '/', '\n']) {
            rest = after;
            continue;
        }
        let Some(end_of_tag) = after.find('>') else {
            break;
        };
        let attributes = &after[..end_of_tag];
        let body_start = end_of_tag + 1;
        let body = if tag == "script" {
            after[body_start..]
                .find(&close)
                .map_or("", |end| &after[body_start..body_start + end])
        } else {
            ""
        };
        found.push((attributes, body));
        rest = &after[body_start..];
    }
    found
}

/// A same-origin absolute path, never a URL with a scheme or host.
fn is_own_origin(reference: &str) -> bool {
    reference.starts_with('/') && !reference.starts_with("//")
}

#[tokio::test]
async fn the_built_page_runs_under_the_policy_it_is_served_with() {
    let app = TestApp::new().await;
    let page = get(&app, "/").await;
    assert_eq!(
        page.status(),
        StatusCode::OK,
        "the UI is not built: run `npm run build` in ui/"
    );
    let policy = page.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(policy.contains("script-src 'self';"), "{policy}");
    let html = text(page).await;

    let scripts = elements(&html, "script");
    assert!(!scripts.is_empty(), "the page loads no script at all");
    for (attributes, body) in &scripts {
        let source = attribute(attributes, "src")
            .unwrap_or_else(|| panic!("an inline script would be blocked: <script{attributes}>"));
        assert!(
            is_own_origin(source),
            "script from another origin: {source}"
        );
        assert!(body.trim().is_empty(), "an external script has a body");
    }

    for (attributes, _) in elements(&html, "link") {
        if let Some(reference) = attribute(attributes, "href") {
            assert!(
                is_own_origin(reference),
                "link to another origin: {reference}"
            );
        }
    }

    let lowered = html.to_ascii_lowercase();
    assert!(!lowered.contains("javascript:"), "a javascript: URL");
    for handler in [
        "onload=",
        "onclick=",
        "onerror=",
        "onsubmit=",
        "onmouseover=",
    ] {
        assert!(
            !lowered.contains(&format!(" {handler}")),
            "inline handler {handler}"
        );
    }
    assert!(
        !lowered.contains("http://") && !lowered.contains("https://"),
        "the page names an absolute URL"
    );
}

#[tokio::test]
async fn every_script_and_stylesheet_the_page_names_is_served_with_the_right_type() {
    let app = TestApp::new().await;
    let html = text(get(&app, "/").await).await;
    let mut checked = 0;
    for (attributes, _) in elements(&html, "script") {
        let source = attribute(attributes, "src").expect("an external script");
        let response = get(&app, source).await;
        assert_eq!(response.status(), StatusCode::OK, "{source}");
        let kind = response.headers()["content-type"].to_str().unwrap();
        assert!(
            kind.contains("javascript"),
            "{source} is served as {kind}, which nosniff would refuse to run"
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        checked += 1;
    }
    for (attributes, _) in elements(&html, "link") {
        if attribute(attributes, "rel") != Some("stylesheet") {
            continue;
        }
        let reference = attribute(attributes, "href").expect("a stylesheet reference");
        let response = get(&app, reference).await;
        assert_eq!(response.status(), StatusCode::OK, "{reference}");
        let kind = response.headers()["content-type"].to_str().unwrap();
        assert!(kind.contains("css"), "{reference} is served as {kind}");
        checked += 1;
    }
    assert!(
        checked >= 2,
        "expected a script and a stylesheet, found {checked} files"
    );
}

#[test]
fn the_element_scanner_finds_tags_and_tells_inline_from_external_scripts() {
    let html = r#"<link rel="stylesheet" href="/a.css"><linked x="1"><script src="/a.js"></script>
        <script>alert(1)</script>"#;
    let links = elements(html, "link");
    assert_eq!(links.len(), 1);
    assert_eq!(attribute(links[0].0, "href"), Some("/a.css"));
    let scripts = elements(html, "script");
    assert_eq!(scripts.len(), 2);
    assert_eq!(attribute(scripts[0].0, "src"), Some("/a.js"));
    assert!(scripts[0].1.is_empty());
    assert_eq!(attribute(scripts[1].0, "src"), None);
    assert_eq!(scripts[1].1, "alert(1)");
    assert!(is_own_origin("/x") && !is_own_origin("//evil/x") && !is_own_origin("https://evil/x"));
}
