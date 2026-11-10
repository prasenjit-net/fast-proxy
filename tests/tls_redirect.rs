//! The plain-HTTP redirect and HSTS, live (M3): real connections to the plain
//! and HTTPS listeners of the real data plane.

mod common;

use common::tls_stack::{Options, Stack};
use fast_proxy_model::{CertificateId, Hsts, HttpsPolicy, RedirectMode, SubsystemId};
use fast_proxy_test_pki::{request, ClientHello, TestCa};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A parsed raw HTTP response.
struct Raw {
    status: u16,
    text: String,
}

impl Raw {
    fn header(&self, name: &str) -> Option<String> {
        self.text.lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_string())
        })
    }
}

fn parse(text: String) -> Raw {
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    Raw { status, text }
}

/// One request to the plain-HTTP listener.
async fn plain(stack: &Stack, host: &str, path: &str) -> Raw {
    let mut stream = TcpStream::connect(stack.http).await.unwrap();
    let raw = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(raw.as_bytes()).await.unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();
    parse(answer)
}

/// One request over TLS to the HTTPS listener.
async fn secure(stack: &Stack, ca: &TestCa, host: &str) -> Raw {
    let hello = ClientHello {
        sni: Some(host.into()),
        trust_pem: vec![ca.pem.clone()],
        ..ClientHello::default()
    };
    let raw = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    parse(request(stack.https, &hello, raw.as_bytes()).await.unwrap())
}

struct Shop {
    site: SubsystemId,
    certificate: CertificateId,
}

async fn shop(stack: &Stack, ca: &TestCa) -> Shop {
    let site = stack.add_site("shop", "shop.example.com").await;
    let certificate = stack
        .serve_with(site, "shop", ca, &["shop.example.com"])
        .await;
    Shop { site, certificate }
}

async fn configure(stack: &Stack, shop: &Shop, redirect: RedirectMode, hsts: Hsts) {
    let https = HttpsPolicy {
        enabled: true,
        certificate_id: Some(shop.certificate),
        redirect,
        hsts,
    };
    stack
        .service
        .set_subsystem_https(shop.site, https)
        .await
        .unwrap();
}

fn hsts(include_subdomains: bool) -> Hsts {
    Hsts {
        enabled: true,
        max_age_secs: 600,
        include_subdomains,
    }
}

#[tokio::test]
async fn plain_http_is_redirected_with_the_path_and_query_kept_byte_for_byte() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let shop = shop(&stack, &ca).await;
    configure(&stack, &shop, RedirectMode::Permanent308, Hsts::default()).await;
    let port = stack.https.port();

    let answer = plain(&stack, "Shop.Example.com", "/a%2Fb/%7e?x=%20&&y").await;
    assert_eq!(answer.status, 308);
    assert_eq!(
        answer.header("location").unwrap(),
        format!("https://shop.example.com:{port}/a%2Fb/%7e?x=%20&&y")
    );
    assert!(answer.header("x-request-id").is_some());
}

#[tokio::test]
async fn mode_301_and_the_public_port_rule() {
    let stack = Stack::start(Options {
        public_port: Some(443),
        ..Options::default()
    })
    .await;
    let ca = TestCa::new("ca");
    let shop = shop(&stack, &ca).await;
    configure(&stack, &shop, RedirectMode::Permanent301, Hsts::default()).await;
    let answer = plain(&stack, "shop.example.com", "/cart?x=1").await;
    assert_eq!(answer.status, 301);
    assert_eq!(
        answer.header("location").unwrap(),
        "https://shop.example.com/cart?x=1"
    );

    let other = Stack::start(Options {
        public_port: Some(8443),
        ..Options::default()
    })
    .await;
    let ca = TestCa::new("ca");
    let shop = self::shop(&other, &ca).await;
    configure(&other, &shop, RedirectMode::Permanent308, Hsts::default()).await;
    let answer = plain(&other, "shop.example.com", "/").await;
    assert_eq!(
        answer.header("location").unwrap(),
        "https://shop.example.com:8443/"
    );
}

#[tokio::test]
async fn an_unknown_host_is_a_404_and_never_a_redirect() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let shop = shop(&stack, &ca).await;
    configure(&stack, &shop, RedirectMode::Permanent308, Hsts::default()).await;
    let answer = plain(&stack, "nobody.example.com", "/").await;
    assert_eq!(answer.status, 404);
    assert!(answer.header("location").is_none());
}

#[tokio::test]
async fn no_redirect_when_off_when_https_is_disabled_or_when_tls_is_off() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let shop = shop(&stack, &ca).await;
    configure(&stack, &shop, RedirectMode::Off, Hsts::default()).await;
    assert_eq!(
        plain(&stack, "shop.example.com", "/").await.status,
        200,
        "redirect off"
    );

    stack.add_site("plain", "plain.example.com").await;
    assert_eq!(
        plain(&stack, "plain.example.com", "/").await.status,
        200,
        "HTTPS not enabled"
    );

    // TLS off for the process: a configured redirect is inert.
    let off = Stack::start(Options {
        tls: false,
        ..Options::default()
    })
    .await;
    let site = off.add_site("shop", "shop.example.com").await;
    let https = HttpsPolicy {
        enabled: true,
        redirect: RedirectMode::Permanent308,
        ..HttpsPolicy::default()
    };
    off.service.set_subsystem_https(site, https).await.unwrap();
    let answer = plain(&off, "shop.example.com", "/").await;
    assert_eq!(
        answer.status, 200,
        "no redirect to a listener that does not exist"
    );
    assert!(answer.header("location").is_none());
}

#[tokio::test]
async fn a_host_with_odd_characters_is_never_turned_into_a_location() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let wild = stack.add_site("wild", "*.wild.example.com").await;
    let certificate = stack
        .serve_with(wild, "wild", &ca, &["*.wild.example.com"])
        .await;
    let https = HttpsPolicy {
        enabled: true,
        certificate_id: Some(certificate),
        redirect: RedirectMode::Permanent308,
        ..HttpsPolicy::default()
    };
    stack
        .service
        .set_subsystem_https(wild, https)
        .await
        .unwrap();

    let normal = plain(&stack, "a.wild.example.com", "/").await;
    assert_eq!(normal.status, 308);
    // The wildcard matches this value, but it is not a plain DNS name.
    let odd = plain(&stack, "evil.com@a.wild.example.com", "/").await;
    assert!(odd.header("location").is_none(), "{}", odd.text);
}

#[tokio::test]
async fn hsts_is_sent_over_https_only() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let shop = shop(&stack, &ca).await;
    configure(&stack, &shop, RedirectMode::Off, hsts(false)).await;
    let secure_answer = secure(&stack, &ca, "shop.example.com").await;
    assert_eq!(secure_answer.status, 200);
    assert_eq!(
        secure_answer.header("strict-transport-security").unwrap(),
        "max-age=600"
    );
    let http_answer = plain(&stack, "shop.example.com", "/").await;
    assert_eq!(http_answer.status, 200);
    assert!(
        http_answer.header("strict-transport-security").is_none(),
        "never over plain HTTP"
    );

    configure(&stack, &shop, RedirectMode::Off, hsts(true)).await;
    let with_subdomains = secure(&stack, &ca, "shop.example.com").await;
    assert_eq!(
        with_subdomains.header("strict-transport-security").unwrap(),
        "max-age=600; includeSubDomains"
    );

    configure(&stack, &shop, RedirectMode::Off, Hsts::default()).await;
    assert!(secure(&stack, &ca, "shop.example.com")
        .await
        .header("strict-transport-security")
        .is_none());
}

#[tokio::test]
async fn the_redirect_and_hsts_follow_live_edits() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let shop = shop(&stack, &ca).await;
    assert_eq!(plain(&stack, "shop.example.com", "/").await.status, 200);
    configure(&stack, &shop, RedirectMode::Permanent308, Hsts::default()).await;
    assert_eq!(plain(&stack, "shop.example.com", "/").await.status, 308);
    configure(&stack, &shop, RedirectMode::Off, Hsts::default()).await;
    assert_eq!(plain(&stack, "shop.example.com", "/").await.status, 200);
}
