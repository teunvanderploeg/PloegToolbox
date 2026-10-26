mod common;

use common::*;
use ploegtoolbox::{
    catalog::{self, APPS},
    store::{Appearance, Settings, is_inside},
};
use std::{fs, path::Path};

#[test]
fn checksum_must_be_complete_hexadecimal_sha256() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    for invalid in [
        String::new(),
        "0".repeat(63),
        "g".repeat(64),
        "0".repeat(65),
    ] {
        let release = release_for(&APPS[0], &root.join("app.zip"), invalid);
        assert!(release.validate().is_err());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn valid_local_release_supports_current_architecture_and_universal() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let mut release = release_for(&APPS[0], &root.join("app.zip"), "0".repeat(64));
    release.validate().unwrap();
    release.arch = "universal".to_owned();
    release.validate().unwrap();
}

#[test]
fn unsupported_architecture_or_unknown_app_cannot_be_installed() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let mut release = release_for(&APPS[0], &root.join("app.zip"), "0".repeat(64));
    release.arch = "unsupported-test-architecture".to_owned();
    assert!(release.validate().is_err());
    release.arch = std::env::consts::ARCH.to_owned();
    release.app = "unrelated-app".to_owned();
    assert!(release.validate().is_err());
}

#[test]
fn network_sources_require_https_and_publisher_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let mut release = release_for(&APPS[0], &root.join("app.zip"), "0".repeat(64));
    for source in [
        "http://example.invalid/app.zip",
        "file:///tmp/app.zip",
        "ftp://example.invalid/app.zip",
    ] {
        release.source = source.to_owned();
        assert!(
            release.validate().is_err(),
            "accepted unsupported scheme {source}"
        );
    }
    release.source = "https://example.invalid/app.zip".to_owned();
    for team in [None, Some("bad\"identity".to_owned()), Some(String::new())] {
        release.publisher_team = team;
        assert!(release.validate().is_err());
    }
}

#[test]
fn local_package_paths_resolve_against_catalog_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let mut release = release_for(&APPS[0], Path::new("packages/sound.zip"), "0".repeat(64));
    release.catalog_dir = root.clone();
    assert_eq!(
        release.local_source().unwrap(),
        root.join("packages/sound.zip")
    );
}

#[test]
fn catalog_load_rejects_unknown_schema_unknown_top_level_fields_and_oversize() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let path = root.join("catalog.json");
    for contents in [
        r#"{"schema":2,"releases":[]}"#,
        r#"{"schema":1,"releases":[],"typo":true}"#,
    ] {
        fs::write(&path, contents).unwrap();
        assert!(catalog::load(&path).is_err());
    }
    fs::write(&path, vec![b' '; 1_048_577]).unwrap();
    assert!(catalog::load(&path).is_err());
}

#[test]
fn semantic_versions_compare_correctly_in_catalog_selection() {
    assert!(catalog::newer("v1.10", "1.9.9"));
    assert!(!catalog::newer("1.0.0-beta.1", "1.0"));
    assert!(!catalog::newer("invalid", "1.0.0"));
    assert!(!catalog::newer("1.0", "1.0.0"));
}

#[test]
fn managed_paths_cannot_point_to_other_directories() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let store = test_store(&root);
    for invalid in ["", "../External", "folder/App", "folder\\App", ".hidden"] {
        assert!(store.managed_path(invalid).is_err());
    }
    assert!(is_inside(
        &store.managed_path("PloegShot").unwrap(),
        &store.apps
    ));
    assert!(!is_inside(
        &root.join("external/PloegShot.app"),
        &store.apps
    ));
    assert!(!is_inside(
        &store.apps.join("nested/PloegShot.app"),
        &store.apps
    ));
}

#[test]
fn malformed_settings_remain_available_for_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let store = test_store(&root);
    let path = store.root.join("settings.json");
    fs::write(&path, b"not-json").unwrap();
    assert!(store.settings().is_err());
    assert_eq!(fs::read(&path).unwrap(), b"not-json");
}

#[cfg(unix)]
#[test]
fn saving_settings_does_not_follow_predictable_temp_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let store = test_store(&root);
    let outside = root.join("private-sentinel.txt");
    fs::write(&outside, b"keep this original content").unwrap();
    std::os::unix::fs::symlink(&outside, store.root.join("settings.json.tmp")).unwrap();
    let settings = Settings {
        appearance: Appearance::Dark,
        catalog: None,
    };
    let result = store.save(&settings);
    assert_eq!(
        fs::read(outside).unwrap(),
        b"keep this original content",
        "settings save followed an external symlink"
    );
    if result.is_ok() {
        assert_eq!(store.settings().unwrap().appearance, Appearance::Dark);
        assert!(!store.root.join("settings.json").is_symlink());
    }
}
