//! Every crate in the workspace takes the shared lints, which forbid `unsafe`
//! code, so a new crate cannot quietly opt out.

use std::path::{Path, PathBuf};

fn manifest(path: &Path) -> toml::Table {
    std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
        .parse()
        .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The root package and every member under `crates/`.
fn package_manifests() -> Vec<PathBuf> {
    let mut manifests = vec![root().join("Cargo.toml")];
    for entry in std::fs::read_dir(root().join("crates")).unwrap() {
        let path = entry.unwrap().path().join("Cargo.toml");
        if path.exists() {
            manifests.push(path);
        }
    }
    manifests
}

#[test]
fn the_workspace_lints_forbid_unsafe_code() {
    let root = manifest(&root().join("Cargo.toml"));
    let unsafe_code = &root["workspace"]["lints"]["rust"]["unsafe_code"];
    assert_eq!(unsafe_code.as_str(), Some("forbid"));
}

#[test]
fn every_crate_takes_the_workspace_lints() {
    let manifests = package_manifests();
    assert!(manifests.len() > 10, "found the member crates");
    for path in manifests {
        let lints = manifest(&path)
            .get("lints")
            .and_then(|lints| lints.get("workspace"))
            .and_then(toml::Value::as_bool);
        assert_eq!(
            lints,
            Some(true),
            "{} needs `[lints] workspace = true`",
            path.display()
        );
    }
}
