#![allow(dead_code)]

use ploegtoolbox::{
    catalog::{App, Release},
    store::Store,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use zip::write::SimpleFileOptions;

pub fn canonical_root(temp: &tempfile::TempDir) -> PathBuf {
    temp.path()
        .canonicalize()
        .expect("canonical test directory")
}

pub fn test_store(root: &Path) -> Store {
    let store = Store {
        root: root.join("state"),
        apps: root.join("managed"),
    };
    fs::create_dir_all(&store.root).unwrap();
    fs::create_dir_all(&store.apps).unwrap();
    store
}

pub fn release_for(app: &App, archive: &Path, hash: String) -> Release {
    Release {
        app: app.id.to_owned(),
        version: "1.0.0".to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        source: archive.to_string_lossy().into_owned(),
        sha256: hash,
        toolbox_open_version: 0,
        publisher_team: None,
        notes: String::new(),
        catalog_dir: PathBuf::new(),
    }
}

pub fn hash(path: &Path) -> String {
    hex::encode(Sha256::digest(fs::read(path).unwrap()))
}

pub fn zip_file(path: &Path, entries: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, content) in entries {
        writer
            .start_file(*name, SimpleFileOptions::default().unix_permissions(0o644))
            .unwrap();
        writer.write_all(content).unwrap();
    }
    writer.finish().unwrap();
}

#[cfg(target_os = "macos")]
pub fn signed_bundle(path: &Path, app: &App, version: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
    fs::create_dir_all(path.join("Contents/Resources")).unwrap();
    let executable = path.join("Contents/MacOS").join(app.name);
    fs::copy("/usr/bin/true", &executable).unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>{name}</string>
<key>CFBundleIdentifier</key><string>{id}</string>
<key>CFBundleName</key><string>{name}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
</dict></plist>"#,
        name = app.name,
        id = app.bundle_id
    );
    fs::write(path.join("Contents/Info.plist"), plist).unwrap();
    fs::write(path.join("Contents/Resources/version-marker.txt"), version).unwrap();
    let output = std::process::Command::new("/usr/bin/codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture signing failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(target_os = "macos")]
pub fn zip_bundle(bundle: &Path, archive: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fn append(writer: &mut zip::ZipWriter<fs::File>, root: &Path, path: &Path) {
        let mut children = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            let relative = child
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if child.is_dir() {
                writer
                    .add_directory(
                        &relative,
                        SimpleFileOptions::default().unix_permissions(0o755),
                    )
                    .unwrap();
                append(writer, root, &child);
            } else {
                writer
                    .start_file(
                        relative,
                        SimpleFileOptions::default()
                            .unix_permissions(fs::metadata(&child).unwrap().permissions().mode()),
                    )
                    .unwrap();
                writer.write_all(&fs::read(child).unwrap()).unwrap();
            }
        }
    }
    let mut writer = zip::ZipWriter::new(fs::File::create(archive).unwrap());
    writer
        .add_directory(
            bundle.file_name().unwrap().to_string_lossy(),
            SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
    append(&mut writer, bundle.parent().unwrap(), bundle);
    writer.finish().unwrap();
}

pub fn zip_with_links(
    path: &Path,
    files: &[(&str, &[u8])],
    links: &[(&str, &str)],
    links_first: bool,
) {
    fn add_links(writer: &mut zip::ZipWriter<fs::File>, links: &[(&str, &str)]) {
        for (name, target) in links {
            writer
                .add_symlink(*name, *target, SimpleFileOptions::default())
                .unwrap();
        }
    }
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    if links_first {
        add_links(&mut writer, links);
    }
    for (name, content) in files {
        writer
            .start_file(*name, SimpleFileOptions::default().unix_permissions(0o644))
            .unwrap();
        writer.write_all(content).unwrap();
    }
    if !links_first {
        add_links(&mut writer, links);
    }
    writer.finish().unwrap();
}
