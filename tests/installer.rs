mod common;

use common::*;
use ploegtoolbox::{catalog::APPS, install};
use std::{
    fs,
    sync::{Arc, Mutex, atomic::AtomicBool},
};
use zip::write::SimpleFileOptions;

#[test]
fn package_hash_mismatch_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let package = root.join("package.zip");
    fs::write(&package, b"package contents").unwrap();
    install::verify_hash(&package, &hash(&package)).unwrap();
    assert!(install::verify_hash(&package, &"0".repeat(64)).is_err());
}

#[test]
fn zip_parent_traversal_and_absolute_paths_cannot_escape_staging() {
    for entry in [
        "../escaped.txt",
        "/absolute-escaped.txt",
        "folder/../../escaped.txt",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = canonical_root(&temp);
        let archive = root.join("unsafe.zip");
        zip_file(&archive, &[(entry, b"unsafe")]);
        let destination = root.join("unpacked");
        fs::create_dir(&destination).unwrap();
        assert!(
            install::extract_zip(&archive, &destination, &AtomicBool::new(false)).is_err(),
            "accepted unsafe path {entry}"
        );
        assert!(!root.join("escaped.txt").exists());
    }
}

#[test]
fn zip_link_target_that_escapes_root_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let archive = root.join("link.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    writer
        .add_symlink(
            "PloegShot.app/Contents/Resources/link",
            "../../../../outside",
            SimpleFileOptions::default(),
        )
        .unwrap();
    writer.finish().unwrap();
    let destination = root.join("unpacked");
    fs::create_dir(&destination).unwrap();
    assert!(install::extract_zip(&archive, &destination, &AtomicBool::new(false)).is_err());
    assert!(
        !destination
            .join("PloegShot.app/Contents/Resources/link")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn extraction_cannot_follow_an_existing_parent_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let archive = root.join("package.zip");
    zip_file(&archive, &[("linked/escaped.txt", b"unsafe")]);
    let destination = root.join("unpacked");
    let outside = root.join("external");
    fs::create_dir(&destination).unwrap();
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, destination.join("linked")).unwrap();
    assert!(install::extract_zip(&archive, &destination, &AtomicBool::new(false)).is_err());
    assert!(!outside.join("escaped.txt").exists());
}

#[test]
fn an_existing_file_cannot_be_overwritten_during_extraction() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let archive = root.join("overwrite.zip");
    zip_file(&archive, &[("same.txt", b"replacement")]);
    let destination = root.join("unpacked");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("same.txt"), b"original").unwrap();
    assert!(install::extract_zip(&archive, &destination, &AtomicBool::new(false)).is_err());
    assert_eq!(fs::read(destination.join("same.txt")).unwrap(), b"original");
}

#[test]
fn cancellation_stops_extraction_before_output_is_created() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let archive = root.join("package.zip");
    zip_file(&archive, &[("document.txt", b"content")]);
    let destination = root.join("unpacked");
    fs::create_dir(&destination).unwrap();
    assert!(install::extract_zip(&archive, &destination, &AtomicBool::new(true)).is_err());
    assert!(!destination.join("document.txt").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn bad_hash_leaves_existing_installation_and_external_apps_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let store = test_store(&root);
    let app = &APPS[2];
    let destination = store.managed_path(app.name).unwrap();
    signed_bundle(&destination, app, "0.9.0");
    let outside = root.join("external/PloegShot.app");
    signed_bundle(&outside, app, "0.8.0");
    let archive = root.join("bad.zip");
    fs::write(&archive, b"not a valid app package").unwrap();
    let release = release_for(app, &archive, "0".repeat(64));
    let error = install::install(
        &store,
        app,
        &release,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    )
    .unwrap_err();
    assert!(error.to_string().contains("checksum"), "{error:#}");
    assert_eq!(
        install::bundle_value(&destination, "CFBundleShortVersionString").unwrap(),
        "0.9.0"
    );
    assert_eq!(
        install::bundle_value(&outside, "CFBundleShortVersionString").unwrap(),
        "0.8.0"
    );
    assert!(!store.recovery_path().exists());
    assert_eq!(fs::read_dir(store.root.join("staging")).unwrap().count(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn local_signed_package_install_discovery_and_removal_work_without_touching_external_copy() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let store = test_store(&root);
    let app = &APPS[2];
    let bundle = root.join("release/PloegShot.app");
    signed_bundle(&bundle, app, "1.0.0");
    let archive = root.join("release.zip");
    zip_bundle(&bundle, &archive);
    let release = release_for(app, &archive, hash(&archive));
    let external = root.join("external/PloegShot.app");
    signed_bundle(&external, app, "0.7.0");
    let events = Mutex::new(Vec::new());
    let destination = install::install(
        &store,
        app,
        &release,
        Arc::new(AtomicBool::new(false)),
        |event| events.lock().unwrap().push(format!("{event:?}")),
    )
    .unwrap();
    assert_eq!(destination, store.managed_path(app.name).unwrap());
    install::verify_bundle(&destination, app, Some(&release)).unwrap();
    let discovered = install::discover(&store, app).unwrap();
    assert!(discovered.managed);
    assert_eq!(discovered.path, destination);
    assert_eq!(discovered.version, "1.0.0");
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event == "Installing")
    );
    install::remove(&store, app).unwrap();
    assert!(!destination.exists());
    assert!(external.exists());
    assert_eq!(
        install::bundle_value(&external, "CFBundleShortVersionString").unwrap(),
        "0.7.0"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn interrupted_update_recovers_backup_and_cleans_incoming_staging() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let store = test_store(&root);
    let app = &APPS[2];
    let backup = store.root.join("backups/shot.app");
    signed_bundle(&backup, app, "0.9.0");
    let destination = store.managed_path(app.name).unwrap();
    signed_bundle(&destination, app, "1.0.0");
    let incoming = store.apps.join(".shot-incoming.app");
    fs::create_dir_all(&incoming).unwrap();
    fs::write(incoming.join("partial"), b"incomplete app").unwrap();
    fs::write(store.recovery_path(), br#"{"app":"shot"}"#).unwrap();
    install::recover(&store).unwrap();
    assert_eq!(
        install::bundle_value(&destination, "CFBundleShortVersionString").unwrap(),
        "0.9.0"
    );
    assert!(!backup.exists());
    assert!(!incoming.exists());
    assert!(!store.recovery_path().exists());
    install::verify_bundle(&destination, app, None).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn removal_rejects_a_symlinked_managed_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let mut store = test_store(&root);
    let external_directory = root.join("external");
    let external = external_directory.join("PloegShot.app");
    signed_bundle(&external, &APPS[2], "1.0.0");
    let alias = root.join("managed-alias");
    std::os::unix::fs::symlink(&external_directory, &alias).unwrap();
    store.apps = alias;
    assert!(install::remove(&store, &APPS[2]).is_err());
    assert!(
        external.exists(),
        "an external app was removed through the managed directory symlink"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn discovery_does_not_claim_external_app_through_symlink_as_managed() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let store = test_store(&root);
    let external = root.join("external/PloegShot.app");
    signed_bundle(&external, &APPS[2], "1.0.0");
    std::os::unix::fs::symlink(&external, store.managed_path(APPS[2].name).unwrap()).unwrap();
    let found = install::discover(&store, &APPS[2]);
    assert!(
        found
            .as_ref()
            .is_none_or(|installation| !installation.managed),
        "external symlink was claimed as a managed install"
    );
    assert!(external.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn recovery_rejects_symlinked_managed_directory_without_replacing_external_app() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let mut store = test_store(&root);
    let external_directory = root.join("external");
    let external = external_directory.join("PloegShot.app");
    signed_bundle(&external, &APPS[2], "1.0.0");
    signed_bundle(&store.root.join("backups/shot.app"), &APPS[2], "0.9.0");
    let alias = root.join("managed-alias");
    std::os::unix::fs::symlink(&external_directory, &alias).unwrap();
    store.apps = alias;
    fs::write(store.recovery_path(), br#"{"app":"shot"}"#).unwrap();
    assert!(install::recover(&store).is_err());
    assert_eq!(
        install::bundle_value(&external, "CFBundleShortVersionString").unwrap(),
        "1.0.0"
    );
    assert!(store.root.join("backups/shot.app").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn recovery_cannot_move_an_external_app_through_a_symlinked_backup_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let store = test_store(&root);
    let app = &APPS[2];
    let external_backups = root.join("external-backups");
    let external = external_backups.join("shot.app");
    signed_bundle(&external, app, "0.9.0");
    let destination = store.managed_path(app.name).unwrap();
    signed_bundle(&destination, app, "1.0.0");
    std::os::unix::fs::symlink(&external_backups, store.root.join("backups")).unwrap();
    fs::write(store.recovery_path(), br#"{"app":"shot"}"#).unwrap();
    assert!(install::recover(&store).is_err());
    assert!(
        external.exists(),
        "recovery moved an external bundle through a backup directory symlink"
    );
    assert_eq!(
        install::bundle_value(&destination, "CFBundleShortVersionString").unwrap(),
        "1.0.0"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn installation_rejects_a_symlinked_staging_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let store = test_store(&root);
    let app = &APPS[2];
    let source = root.join("release/PloegShot.app");
    signed_bundle(&source, app, "1.0.0");
    let archive = root.join("release.zip");
    zip_bundle(&source, &archive);
    let release = release_for(app, &archive, hash(&archive));
    let external_staging = root.join("external-staging");
    fs::create_dir(&external_staging).unwrap();
    fs::write(external_staging.join("sentinel.txt"), b"preserve this").unwrap();
    std::os::unix::fs::symlink(&external_staging, store.root.join("staging")).unwrap();
    let result = install::install(
        &store,
        app,
        &release,
        Arc::new(AtomicBool::new(false)),
        |_| {},
    );
    assert!(result.is_err());
    assert!(!store.managed_path(app.name).unwrap().exists());
    assert_eq!(
        fs::read(external_staging.join("sentinel.txt")).unwrap(),
        b"preserve this"
    );
}

#[cfg(unix)]
#[test]
fn standard_framework_symlink_chains_are_preserved_in_any_archive_order() {
    for links_first in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let root = canonical_root(&temp);
        let archive = root.join("framework.zip");
        let framework = "SoundPloeg.app/Contents/Frameworks/SoundPloegCore.framework";
        let executable = format!("{framework}/Versions/A/SoundPloegCore");
        let info = format!("{framework}/Versions/A/Resources/Info.plist");
        let resources_link = format!("{framework}/Resources");
        let binary_link = format!("{framework}/SoundPloegCore");
        let current_link = format!("{framework}/Versions/Current");
        zip_with_links(
            &archive,
            &[
                (&executable, b"framework binary"),
                (&info, b"framework resources"),
            ],
            &[
                (&resources_link, "Versions/Current/Resources"),
                (&binary_link, "Versions/Current/SoundPloegCore"),
                (&current_link, "A"),
            ],
            links_first,
        );
        let destination = root.join("unpacked");
        fs::create_dir(&destination).unwrap();
        install::extract_zip(&archive, &destination, &AtomicBool::new(false)).unwrap();
        for name in [&resources_link, &binary_link, &current_link] {
            assert!(
                destination.join(name).is_symlink(),
                "framework link was flattened: {name}"
            );
            assert!(
                destination
                    .join(name)
                    .canonicalize()
                    .unwrap()
                    .starts_with(&destination)
            );
        }
        assert_eq!(
            fs::read_link(destination.join(&current_link)).unwrap(),
            std::path::PathBuf::from("A")
        );
        assert_eq!(
            fs::read(destination.join(&resources_link).join("Info.plist")).unwrap(),
            b"framework resources"
        );
        assert_eq!(
            fs::read(destination.join(&binary_link)).unwrap(),
            b"framework binary"
        );
    }
}

#[cfg(unix)]
#[test]
fn internal_parent_relative_symlink_targets_are_supported() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let archive = root.join("relative-link.zip");
    zip_with_links(
        &archive,
        &[("App.app/Versions/A/data.txt", b"payload")],
        &[("App.app/Links/data.txt", "../Versions/A/data.txt")],
        true,
    );
    let destination = root.join("unpacked");
    install::extract_zip(&archive, &destination, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        fs::read(destination.join("App.app/Links/data.txt")).unwrap(),
        b"payload"
    );
}

#[test]
fn archive_writes_below_queued_symlinks_are_rejected_before_any_file_is_written() {
    for links_first in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let root = canonical_root(&temp);
        let archive = root.join("link-parent.zip");
        zip_with_links(
            &archive,
            &[
                ("real/file.txt", b"original"),
                ("alias/injected.txt", b"must not be written"),
            ],
            &[("alias", "real")],
            links_first,
        );
        let destination = root.join("unpacked");
        install::extract_zip(&archive, &destination, &AtomicBool::new(false)).unwrap_err();
        assert!(!destination.join("real/file.txt").exists());
        assert!(!destination.join("alias").is_symlink());
        assert!(!destination.join("real/injected.txt").exists());
    }
}

#[test]
fn absolute_and_drive_absolute_symlink_targets_are_rejected() {
    for target in [
        "/tmp/outside",
        "C:/Windows/System32",
        "C:\\Windows\\System32",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = canonical_root(&temp);
        let archive = root.join("absolute-link.zip");
        zip_with_links(&archive, &[], &[("link", target)], true);
        let destination = root.join("unpacked");
        assert!(
            install::extract_zip(&archive, &destination, &AtomicBool::new(false)).is_err(),
            "accepted link target {target}"
        );
        assert!(!destination.join("link").is_symlink());
    }
}

#[cfg(unix)]
#[test]
fn dangling_symlinks_are_rejected_and_removed() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let archive = root.join("dangling-link.zip");
    zip_with_links(&archive, &[], &[("link", "does-not-exist")], true);
    let destination = root.join("unpacked");
    assert!(install::extract_zip(&archive, &destination, &AtomicBool::new(false)).is_err());
    assert!(!destination.join("link").is_symlink());
}

#[cfg(unix)]
#[test]
fn cyclic_symlink_chains_are_rejected_and_removed() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let archive = root.join("cycle.zip");
    zip_with_links(&archive, &[], &[("a", "b"), ("b", "a")], true);
    let destination = root.join("unpacked");
    assert!(install::extract_zip(&archive, &destination, &AtomicBool::new(false)).is_err());
    assert!(!destination.join("a").is_symlink());
    assert!(!destination.join("b").is_symlink());
}

#[cfg(unix)]
#[test]
fn a_lexically_internal_chain_cannot_resolve_outside_the_extraction_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let outside = root.join("outside.txt");
    fs::write(&outside, b"preserve external data").unwrap();
    let archive = root.join("escaping-chain.zip");
    zip_with_links(
        &archive,
        &[("elsewhere/payload.txt", b"payload")],
        &[
            ("a/b/alias", "../../elsewhere"),
            ("escape", "a/b/alias/../../outside.txt"),
        ],
        true,
    );
    let destination = root.join("unpacked");
    assert!(install::extract_zip(&archive, &destination, &AtomicBool::new(false)).is_err());
    assert_eq!(fs::read(outside).unwrap(), b"preserve external data");
    assert!(!destination.join("a/b/alias").is_symlink());
    assert!(!destination.join("escape").is_symlink());
}

#[cfg(unix)]
#[test]
fn directory_backlink_cycles_are_rejected_even_when_individual_links_resolve() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let archive = root.join("directory-cycle.zip");
    zip_with_links(
        &archive,
        &[("A/a.txt", b"a"), ("B/b.txt", b"b")],
        &[("A/to-B", "../B"), ("B/to-A", "../A")],
        true,
    );
    let destination = root.join("unpacked");
    assert!(install::extract_zip(&archive, &destination, &AtomicBool::new(false)).is_err());
    assert!(!destination.join("A/to-B").is_symlink());
    assert!(!destination.join("B/to-A").is_symlink());
}

#[test]
fn normalized_file_and_link_path_conflicts_are_rejected_before_writing() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let archive = root.join("duplicate-path.zip");
    zip_with_links(
        &archive,
        &[("same.txt", b"regular file")],
        &[("folder/../same.txt", "real.txt")],
        false,
    );
    let destination = root.join("unpacked");
    assert!(install::extract_zip(&archive, &destination, &AtomicBool::new(false)).is_err());
    assert!(!destination.join("same.txt").exists());
}

#[test]
fn same_version_build_with_window_support_is_an_update_for_legacy_installations() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let mut release = release_for(&APPS[0], &root.join("new.zip"), "0".repeat(64));
    release.toolbox_open_version = 1;
    let mut installed = install::Installation {
        path: root.join("external/SoundPloeg.app"),
        version: "1.0".into(),
        managed: false,
        toolbox_open_version: 0,
    };
    assert!(install::update_available(&installed, &release));
    installed.toolbox_open_version = 1;
    assert!(!install::update_available(&installed, &release));
    installed.version = "2.0".into();
    installed.toolbox_open_version = 0;
    assert!(
        !install::update_available(&installed, &release),
        "window support must not trigger a version downgrade"
    );
}

#[test]
fn opening_a_removed_app_returns_a_recoverable_error() {
    let temp = tempfile::tempdir().unwrap();
    let error = install::open_app(&temp.path().join("missing.app"), &APPS[0]).unwrap_err();
    assert!(error.to_string().contains("Refresh the app list"));
}

#[cfg(target_os = "macos")]
#[test]
fn opening_the_wrong_bundle_is_rejected_before_launch() {
    let temp = tempfile::tempdir().unwrap();
    let bundle = canonical_root(&temp).join("Wrong.app");
    signed_bundle(&bundle, &APPS[2], "1.0.0");
    let error = install::open_app(&bundle, &APPS[0]).unwrap_err();
    assert!(error.to_string().contains("not SoundPloeg"));
}

#[cfg(target_os = "macos")]
#[test]
fn catalog_window_support_is_verified_against_the_signed_app_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let root = canonical_root(&temp);
    let store = test_store(&root);
    let bundle = store.managed_path(APPS[2].name).unwrap();
    signed_bundle(&bundle, &APPS[2], "1.0.0");
    let mut release = release_for(&APPS[2], &root.join("new.zip"), "0".repeat(64));
    release.toolbox_open_version = 1;
    assert_eq!(install::toolbox_open_version(&bundle), 0);
    let error = install::verify_bundle(&bundle, &APPS[2], Some(&release)).unwrap_err();
    assert!(error.to_string().contains("opening behavior"));
    let info = bundle.join("Contents/Info.plist");
    let contents = fs::read_to_string(&info).unwrap().replace(
        "</dict>",
        "<key>PloegToolboxOpenVersion</key><integer>1</integer></dict>",
    );
    fs::write(info, contents).unwrap();
    let signed = std::process::Command::new("/usr/bin/codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(&bundle)
        .output()
        .unwrap();
    assert!(signed.status.success());
    install::verify_bundle(&bundle, &APPS[2], Some(&release)).unwrap();
    let discovered = install::discover(&store, &APPS[2]).unwrap();
    assert_eq!(discovered.toolbox_open_version, 1);
    assert!(!install::update_available(&discovered, &release));
}
