//! ACME with two instances sharing one database (M4 exit criterion 5): one
//! holds the lease and talks to the CA; the CA's fetches reach both and both
//! answer; the certificate reaches both through the sync poll; when the holder
//! dies without releasing the lease, the other takes over once it expires.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::acme_binary::{config_lines, trusted_get, turn_acme_on, wait_for_status};
use common::{admin, create_site, free_port, start_backend, start_on_free_ports, Proxy};
use fast_proxy_test_acme::TestAcme;
use serde_json::json;

const SHOP: &str = "shop.example.com";
const WWW: &str = "www.shop.example.com";

fn instance(dir: &Path, shared: &Path, ca: &TestAcme) -> (Proxy, u16) {
    let https_port = free_port();
    let extra = format!(
        "database = {db:?}\nmaster_key_file = {key:?}\nconfig_poll_interval = \"200ms\"",
        db = shared.join("fast-proxy.db"),
        key = shared.join("master.key"),
    );
    let lines = config_lines(ca, https_port, &extra)
        .replace("propagation_wait = \"0s\"", "propagation_wait = \"1s\"")
        + "lease_ttl = \"30s\"\n";
    (start_on_free_ports(dir, &lines, ""), https_port)
}

fn holds_lease(proxy: &Proxy) -> bool {
    admin(proxy, "GET", "/api/acme", None).1["data"]["lease"]["heldHere"] == true
}

#[test]
fn one_instance_orders_both_answer_and_serve_and_the_other_takes_over_when_it_dies() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let ca = runtime.block_on(TestAcme::start());
    let (shared, dir_a, dir_b) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    // One after the other, so the first creates the schema and the master key.
    let (a, https_a) = instance(dir_a.path(), shared.path(), &ca);
    let (b, https_b) = instance(dir_b.path(), shared.path(), &ca);
    ca.set_challenge_targets(vec![a.data, b.data]);

    let backend = start_backend("shop-backend");
    let (id, _) = create_site(&a, "shop", SHOP, backend);
    let (status, body) = admin(
        &a,
        "POST",
        &format!("/api/subsystems/{id}/hosts"),
        Some(json!({ "pattern": WWW })),
    );
    assert_eq!(status, 201, "{body}");
    turn_acme_on(&a, id, "shop");

    let patience = Duration::from_secs(30);
    wait_for_status(&a, "shop", patience, |s| s["state"] == "valid");
    // Both instances answered a fetch, and only one placed orders.
    let validations = ca.validations();
    assert_eq!(validations.len(), 2, "{validations:?}");
    assert!(validations.iter().all(|v| v.ok), "{validations:?}");
    assert_ne!(
        validations[0].target, validations[1].target,
        "both instances answered"
    );
    assert_eq!(ca.orders(), 1);
    assert_eq!(ca.accounts(), 1);
    assert_ne!(
        holds_lease(&a),
        holds_lease(&b),
        "exactly one holds the lease"
    );

    // The certificate reaches both instances.
    for (port, name) in [(https_a, "a"), (https_b, "b")] {
        let addr = ([127, 0, 0, 1], port).into();
        let deadline = Instant::now() + patience;
        loop {
            let answer = trusted_get(&runtime, addr, WWW, &ca.issuer_pem);
            if answer.starts_with("HTTP/1.1 200") {
                break;
            }
            assert!(Instant::now() < deadline, "instance {name}: {answer}");
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    // The holder dies without releasing the lease; the survivor takes over
    // once it expires and orders for a new host.
    let (mut holder, survivor) = if holds_lease(&a) { (a, b) } else { (b, a) };
    holder.child.kill().unwrap();
    let _ = holder.child.wait();
    ca.set_challenge_targets(vec![survivor.data]);
    let (status, body) = admin(
        &survivor,
        "POST",
        &format!("/api/subsystems/{id}/hosts"),
        Some(json!({ "pattern": "api.shop.example.com" })),
    );
    assert_eq!(status, 201, "{body}");
    let taken_over = wait_for_status(&survivor, "shop", Duration::from_secs(90), |s| {
        s["state"] == "valid"
            && s["eligibleNames"]
                .as_array()
                .is_some_and(|names| names.len() == 3)
            && s["lastSuccessAtMs"].as_i64() > Some(0)
            && s["certificateId"].is_i64()
    });
    assert_eq!(ca.orders(), 2, "{taken_over}");
    assert!(holds_lease(&survivor));
}
