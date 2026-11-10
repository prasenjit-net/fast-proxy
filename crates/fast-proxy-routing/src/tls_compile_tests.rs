//! Compiling HTTPS settings, certificates and upstream CA names into snapshots.

use std::collections::HashMap;
use std::sync::Arc;

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_model::{
    CertificateId, CertificateName, CertificateSource, Config, HostPattern, HttpsPolicy,
    RedirectMode,
};
use fast_proxy_test_pki::TestCa;
use fast_proxy_tls::{
    generate_self_signed, parse_chain, read_metadata, CaBundle, CertCache, MasterKey,
    StoredCertificate, Vault,
};

use crate::compile::tests::{group, subsystem};
use crate::{compile_with_tls, CompileError, Snapshot, SniLookup, TlsInputs};

const KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const NOW: i64 = 1_800_000_000_000;

fn vault() -> Vault {
    Vault::new(&MasterKey::from_base64(KEY).unwrap())
}

fn stored(id: i64, name: &str, names: &[&str], updated: i64) -> StoredCertificate {
    let names: Vec<String> = names.iter().map(ToString::to_string).collect();
    let generated = generate_self_signed(&names, NOW).unwrap();
    let metadata = read_metadata(&parse_chain(&generated.chain_pem).unwrap()[0]).unwrap();
    StoredCertificate {
        id: CertificateId(id),
        name: CertificateName::parse(name).unwrap(),
        source: CertificateSource::SelfSigned,
        sealed_key: vault()
            .seal_key(CertificateId(id), name, &generated.key.to_sealed_bytes())
            .unwrap(),
        chain_pem: generated.chain_pem,
        metadata,
        updated_at_ms: updated,
    }
}

fn cache(rows: &[StoredCertificate], with_vault: bool) -> CertCache {
    let vault = vault();
    CertCache::empty()
        .rebuild(rows, with_vault.then_some(&vault))
        .0
}

fn tls(enabled: bool, certificates: CertCache) -> TlsInputs {
    TlsInputs {
        enabled,
        certificates,
        ca_bundles: HashMap::new(),
        client_configs: Arc::default(),
    }
}

fn bundles(entries: &[(&str, &str, i64)]) -> HashMap<String, CaBundle> {
    entries
        .iter()
        .map(|(name, pem, updated)| {
            (
                name.to_string(),
                CaBundle {
                    name: name.to_string(),
                    pem: pem.to_string(),
                    updated_at_ms: *updated,
                },
            )
        })
        .collect()
}

fn tls_group_config(ca: &str, enabled: bool, verify: bool) -> Config {
    let mut config = config_with(None, false);
    let mut backend = group(10, "backend", "127.0.0.1:3000");
    backend.policy.tls.enabled = enabled;
    backend.policy.tls.verify = verify;
    backend.policy.tls.ca = ca.into();
    config.subsystems[0].upstream_groups = vec![backend];
    config
}

fn client_tls(snapshot: &Snapshot) -> Option<Arc<rustls::ClientConfig>> {
    let upstream = snapshot
        .subsystems
        .values()
        .next()?
        .upstreams
        .values()
        .next()?;
    upstream.client_tls.clone()
}

fn config_with(certificate: Option<i64>, https_enabled: bool) -> Config {
    let mut shop = subsystem(1, "shop", &["shop.example.com", "*.shop.example.com"]);
    shop.https = HttpsPolicy {
        enabled: https_enabled,
        certificate_id: certificate.map(CertificateId),
        redirect: if https_enabled {
            RedirectMode::Permanent308
        } else {
            RedirectMode::Off
        },
        ..HttpsPolicy::default()
    };
    Config {
        subsystems: vec![shop],
    }
}

fn compile(config: &Config, tls: &TlsInputs) -> Result<Snapshot, CompileError> {
    compile_with_tls(1, config, &RuntimeRegistry::new(), tls)
}

#[test]
fn with_tls_off_https_settings_are_inert_and_unchecked() {
    let snapshot = compile(&config_with(None, true), &tls(false, CertCache::empty())).unwrap();
    assert!(!snapshot.tls_enabled);
    let https = &snapshot.subsystems.values().next().unwrap().https;
    assert!(https.enabled && https.certificate.is_none());
}

#[test]
fn https_without_a_certificate_is_rejected_while_tls_is_on() {
    let err = compile(&config_with(None, true), &tls(true, CertCache::empty())).unwrap_err();
    assert_eq!(
        err,
        CompileError::HttpsWithoutCertificate {
            subsystem: "shop".into()
        }
    );
}

#[test]
fn a_reference_to_a_missing_certificate_is_rejected() {
    let err = compile(&config_with(Some(9), true), &tls(true, CertCache::empty())).unwrap_err();
    assert_eq!(
        err,
        CompileError::UnknownCertificate {
            subsystem: "shop".into(),
            id: 9
        }
    );
    assert!(err.to_string().contains("certificate 9"));
}

#[test]
fn a_subsystem_without_https_needs_no_certificate() {
    let snapshot = compile(&config_with(None, false), &tls(true, CertCache::empty())).unwrap();
    assert!(snapshot.tls_enabled);
}

#[test]
fn the_certificate_handle_travels_in_the_snapshot() {
    let rows = [stored(7, "shop", &["shop.example.com"], 1)];
    let certificates = cache(&rows, true);
    let snapshot = compile(
        &config_with(Some(7), true),
        &tls(true, certificates.clone()),
    )
    .unwrap();
    let https = &snapshot.subsystems.values().next().unwrap().https;
    let handle = https.certificate.as_ref().expect("handle attached");
    assert_eq!(handle.name.as_str(), "shop");
    assert_eq!(https.certificate_id, Some(CertificateId(7)));
    assert_eq!(https.redirect, RedirectMode::Permanent308);
    assert!(Arc::ptr_eq(
        handle,
        certificates.handle(CertificateId(7)).unwrap()
    ));
}

#[test]
fn an_unreadable_certificate_compiles_and_leaves_no_handle() {
    let rows = [stored(7, "shop", &["shop.example.com"], 1)];
    let snapshot = compile(&config_with(Some(7), true), &tls(true, cache(&rows, false))).unwrap();
    let https = &snapshot.subsystems.values().next().unwrap().https;
    assert_eq!(https.certificate_id, Some(CertificateId(7)));
    assert!(https.certificate.is_none());
}

#[test]
fn a_new_compile_swaps_the_certificate_handle() {
    let before = cache(&[stored(7, "shop", &["shop.example.com"], 1)], true);
    let first = compile(&config_with(Some(7), true), &tls(true, before.clone())).unwrap();
    let renewed = before
        .rebuild(
            &[stored(7, "shop", &["shop.example.com"], 2)],
            Some(&vault()),
        )
        .0;
    let second = compile(&config_with(Some(7), true), &tls(true, renewed)).unwrap();
    let handle = |snapshot: &Snapshot| {
        Arc::clone(
            snapshot
                .subsystems
                .values()
                .next()
                .unwrap()
                .https
                .certificate
                .as_ref()
                .unwrap(),
        )
    };
    assert!(!Arc::ptr_eq(&handle(&first), &handle(&second)));
    assert_ne!(handle(&first).version, handle(&second).version);
}

#[test]
fn sni_resolves_exact_wildcard_and_unknown_names() {
    let rows = [stored(7, "shop", &["shop.example.com"], 1)];
    let snapshot = compile(&config_with(Some(7), true), &tls(true, cache(&rows, true))).unwrap();
    for name in ["shop.example.com", "SHOP.example.com", "a.shop.example.com"] {
        let SniLookup::Subsystem { https, .. } = snapshot.sni_lookup(name) else {
            panic!("{name} should resolve");
        };
        assert!(https.certificate.is_some(), "{name}");
    }
    for name in ["other.example.com", "example.com", ""] {
        assert!(
            matches!(snapshot.sni_lookup(name), SniLookup::Unknown),
            "{name}"
        );
    }
}

#[test]
fn an_unknown_ca_bundle_name_is_rejected_and_a_known_one_accepted() {
    let mut config = config_with(None, false);
    let mut backend = group(10, "backend", "127.0.0.1:3000");
    backend.policy.tls.enabled = true;
    backend.policy.tls.ca = "corp".into();
    config.subsystems[0].upstream_groups = vec![backend];
    let err = compile(&config, &tls(false, CertCache::empty())).unwrap_err();
    assert_eq!(
        err,
        CompileError::UnknownCaBundle {
            subsystem: "shop".into(),
            upstream: "backend".into(),
            name: "corp".into()
        }
    );
    let known = TlsInputs {
        ca_bundles: bundles(&[("corp", &TestCa::new("corp root").pem, 1)]),
        ..TlsInputs::off()
    };
    compile(&config, &known).unwrap();
}

#[test]
fn unrelated_edits_keep_endpoint_runtime_state() {
    let rows = [stored(7, "shop", &["shop.example.com"], 1)];
    let inputs = tls(true, cache(&rows, true));
    let config = config_with(Some(7), true);
    let first = compile(&config, &inputs).unwrap();
    let mut edited = config.clone();
    edited.subsystems[0].description = "changed".into();
    edited.subsystems[0]
        .hosts
        .push(HostPattern::parse("extra.example.com").unwrap());
    let second = compile_with_tls(2, &edited, &first.runtime_registry(), &inputs).unwrap();
    let runtime = |snapshot: &Snapshot| {
        let upstream = snapshot
            .subsystems
            .values()
            .next()
            .unwrap()
            .upstreams
            .values()
            .next()
            .unwrap()
            .clone();
        Arc::clone(&upstream.candidates[0].runtime)
    };
    assert!(Arc::ptr_eq(&runtime(&first), &runtime(&second)));
}

#[test]
fn a_tls_group_gets_a_client_configuration_and_a_plain_group_none() {
    let inputs = TlsInputs {
        ca_bundles: bundles(&[("corp", &TestCa::new("corp root").pem, 1)]),
        ..TlsInputs::off()
    };
    let secure = compile(&tls_group_config("corp", true, true), &inputs).unwrap();
    let config = client_tls(&secure).expect("a TLS group carries its configuration");
    assert!(config.alpn_protocols.is_empty(), "the connector sets ALPN");
    let plain = compile(&tls_group_config("corp", false, true), &inputs).unwrap();
    assert!(client_tls(&plain).is_none());
    let unverified = compile(&tls_group_config("", true, false), &inputs).unwrap();
    assert!(client_tls(&unverified).is_some());
}

#[test]
fn a_replaced_bundle_compiles_to_a_new_configuration_and_an_unchanged_one_is_reused() {
    let first_pem = TestCa::new("first").pem;
    let first = TlsInputs {
        ca_bundles: bundles(&[("corp", &first_pem, 1)]),
        ..TlsInputs::off()
    };
    let config = tls_group_config("corp", true, true);
    let one = client_tls(&compile(&config, &first).unwrap()).unwrap();
    let again = client_tls(&compile(&config, &first).unwrap()).unwrap();
    assert!(
        Arc::ptr_eq(&one, &again),
        "unchanged bundle, same configuration"
    );

    let replaced = TlsInputs {
        ca_bundles: bundles(&[("corp", &TestCa::new("second").pem, 2)]),
        client_configs: first.client_configs.clone(),
        ..TlsInputs::off()
    };
    let two = client_tls(&compile(&config, &replaced).unwrap()).unwrap();
    assert!(!Arc::ptr_eq(&one, &two));
}

#[test]
fn an_unusable_bundle_is_a_compile_error_naming_the_upstream() {
    let inputs = TlsInputs {
        ca_bundles: bundles(&[("corp", "no certificates here", 1)]),
        ..TlsInputs::off()
    };
    let err = compile(&tls_group_config("corp", true, true), &inputs).unwrap_err();
    assert!(
        matches!(&err, CompileError::InvalidUpstream { upstream, .. } if upstream == "backend"),
        "{err}"
    );
}
