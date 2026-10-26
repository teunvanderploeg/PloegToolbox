use crate::{
    catalog::{App, Release},
    store::{Store, ensure_no_symlink_ancestors, is_inside},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug)]
pub struct Installation {
    pub path: PathBuf,
    pub version: String,
    pub managed: bool,
    pub toolbox_open_version: u32,
}

#[derive(Clone, Debug)]
pub enum Progress {
    Downloading(u64, u64),
    Verifying,
    Installing,
    Removing,
    Complete(PathBuf),
    Removed,
    Failed(String),
    Cancelled,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    app: String,
}

fn run(mut command: Command, operation: &str) -> Result<String> {
    let output = command.output().with_context(|| operation.to_owned())?;
    ensure!(
        output.status.success(),
        "{operation}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub fn bundle_value(path: &Path, key: &str) -> Result<String> {
    let mut c = Command::new("/usr/bin/plutil");
    c.args(["-extract", key, "raw", "-o", "-"])
        .arg(path.join("Contents/Info.plist"));
    run(c, "Read app information")
}

pub fn discover(store: &Store, app: &App) -> Option<Installation> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let managed = store.managed_path(app.name).ok()?;
    let mut locations = vec![(
        PathBuf::from("/Applications").join(format!("{}.app", app.name)),
        false,
    )];
    if ensure_no_symlink_ancestors(&store.apps).is_ok() && !managed.is_symlink() {
        locations.insert(0, (managed, true));
    }
    if let Some(base) = directories::BaseDirs::new() {
        locations.push((
            base.home_dir()
                .join("Applications")
                .join(format!("{}.app", app.name)),
            false,
        ));
    }
    locations.into_iter().find_map(|(path, managed)| {
        if bundle_value(&path, "CFBundleIdentifier").ok()?.as_str() != app.bundle_id {
            return None;
        }
        Some(Installation {
            version: bundle_value(&path, "CFBundleShortVersionString")
                .unwrap_or_else(|_| "Unknown".into()),
            toolbox_open_version: toolbox_open_version(&path),
            path,
            managed,
        })
    })
}

pub fn toolbox_open_version(path: &Path) -> u32 {
    bundle_value(path, "PloegToolboxOpenVersion")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

pub fn update_available(installed: &Installation, release: &Release) -> bool {
    crate::catalog::newer(&release.version, &installed.version)
        || (crate::catalog::normalized_version(&release.version)
            == crate::catalog::normalized_version(&installed.version)
            && release.toolbox_open_version > installed.toolbox_open_version)
}

/// Launch one registered app without starting a duplicate process. The argument
/// handles cold launches; the app's standard reopen handler handles warm ones.
pub fn open_app(path: &Path, app: &App) -> Result<()> {
    ensure!(
        path.is_dir(),
        "{} is no longer installed. Refresh the app list.",
        app.name
    );
    ensure!(
        bundle_value(path, "CFBundleIdentifier")? == app.bundle_id,
        "The installed bundle is not {}. Refresh the app list.",
        app.name
    );
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("/usr/bin/open");
        c.arg("-a").arg(path).args(["--args", "--show-settings"]);
        run(c, &format!("Open {}", app.name))?;
    }
    #[cfg(not(target_os = "macos"))]
    open(path)?;
    Ok(())
}

pub fn open(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("/usr/bin/open");
        c.arg(path);
        run(c, "Open app")?;
    }
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer.exe").arg(path).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

pub fn is_running(path: &Path) -> bool {
    let system = sysinfo::System::new_all();
    system
        .processes()
        .values()
        .any(|p| p.exe().is_some_and(|p| p.starts_with(path)))
}

pub fn verify_hash(path: &Path, expected: &str) -> Result<()> {
    let mut input = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    ensure!(
        hex::encode(hash.finalize()).eq_ignore_ascii_case(expected),
        "Package checksum did not match. The installed app was not changed."
    );
    Ok(())
}

pub fn extract_zip(archive: &Path, destination: &Path, cancelled: &AtomicBool) -> Result<()> {
    use std::collections::{HashMap, HashSet};
    use std::path::Component;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Directory,
        File,
        Link,
    }
    struct Entry {
        index: usize,
        path: PathBuf,
        kind: Kind,
        mode: u32,
        size: u64,
    }

    fn relative_path(path: &Path) -> Result<PathBuf> {
        let text = path.to_string_lossy();
        ensure!(
            !text.contains(['\\', '\0']),
            "Package contains an unsafe path"
        );
        let bytes = text.as_bytes();
        ensure!(
            !(bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'),
            "Package contains an absolute drive path"
        );
        let mut result = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Normal(name) => result.push(name),
                Component::CurDir => {}
                Component::ParentDir => {
                    ensure!(result.pop(), "Package path escapes the extraction root")
                }
                Component::RootDir | Component::Prefix(_) => {
                    bail!("Package contains an absolute path")
                }
            }
        }
        Ok(result)
    }

    ensure!(!cancelled.load(Ordering::Relaxed), "Cancelled");
    ensure_no_symlink_ancestors(destination)?;
    fs::create_dir_all(destination)?;
    let root = destination
        .canonicalize()
        .context("Cannot resolve extraction root")?;
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;
    ensure!(zip.len() <= 100_000, "Package contains too many files");
    let mut total = 0u64;
    let mut entries = Vec::with_capacity(zip.len());
    let mut kinds = HashMap::new();
    let mut links = Vec::new();

    // Validate the complete archive before writing. A later link entry must not
    // turn an earlier file's parent into a link, regardless of archive order.
    for index in 0..zip.len() {
        ensure!(!cancelled.load(Ordering::Relaxed), "Cancelled");
        let mut file = zip.by_index(index)?;
        let enclosed = file
            .enclosed_name()
            .context("Package contains an unsafe file path")?;
        let path = relative_path(&enclosed)?;
        ensure!(
            !path.as_os_str().is_empty(),
            "Package contains an empty file path"
        );
        let mode = file.unix_mode().unwrap_or(0);
        let file_type = mode & 0o170000;
        ensure!(
            matches!(file_type, 0 | 0o100000 | 0o040000 | 0o120000),
            "Package contains an unsupported file type"
        );
        let kind = if file_type == 0o120000 {
            Kind::Link
        } else if file.is_dir() {
            Kind::Directory
        } else {
            Kind::File
        };
        ensure!(
            kinds.insert(path.clone(), kind).is_none(),
            "Package contains conflicting duplicate paths"
        );
        total = total
            .checked_add(file.size())
            .context("Package size overflow")?;
        ensure!(
            total <= 8 * 1024 * 1024 * 1024,
            "Expanded package exceeds the 8 GB limit"
        );
        if kind == Kind::Link {
            ensure!(
                file.size() > 0 && file.size() <= 4096,
                "Package symbolic-link target is invalid or too long"
            );
            let mut bytes = Vec::new();
            let expected = file.size();
            (&mut file).take(expected + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 == expected,
                "Symbolic-link size differs from package metadata"
            );
            let target = PathBuf::from(
                String::from_utf8(bytes).context("Package symbolic-link target is not UTF-8")?,
            );
            // Check the target itself, then its position relative to its link.
            // Parent components are valid for internal framework links only when
            // the complete joined path stays within this extraction root.
            ensure!(
                !target.is_absolute(),
                "Package symbolic links must be relative"
            );
            let text = target.to_string_lossy();
            let target_bytes = text.as_bytes();
            ensure!(
                !(text.contains(['\\', '\0'])
                    || (target_bytes.len() >= 2
                        && target_bytes[0].is_ascii_alphabetic()
                        && target_bytes[1] == b':')),
                "Package symbolic-link target is unsafe"
            );
            let joined = path.parent().unwrap_or(Path::new("")).join(&target);
            relative_path(&joined)?;
            links.push((path.clone(), target));
        }
        entries.push(Entry {
            index,
            path,
            kind,
            mode,
            size: file.size(),
        });
    }

    for entry in &entries {
        let mut parent = entry.path.parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            ensure!(
                !kinds.get(path).is_some_and(|kind| *kind != Kind::Directory),
                "Package entry is nested beneath a file or symbolic link"
            );
            parent = path.parent();
        }
        let output = root.join(&entry.path);
        ensure_no_symlink_ancestors(&output)?;
        if let Ok(metadata) = fs::symlink_metadata(&output) {
            ensure!(
                entry.kind == Kind::Directory && metadata.is_dir(),
                "Package output already exists"
            );
        }
    }

    // No symbolic links exist yet, so extraction never writes through a link
    // introduced by this archive. Existing link parents are checked again.
    for entry in entries.iter().filter(|entry| entry.kind != Kind::Link) {
        ensure!(!cancelled.load(Ordering::Relaxed), "Cancelled");
        let output = root.join(&entry.path);
        ensure_no_symlink_ancestors(&output)?;
        if entry.kind == Kind::Directory {
            fs::create_dir_all(&output)?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        let mut file = zip.by_index(entry.index)?;
        let copied = std::io::copy(
            &mut (&mut file).take(entry.size.saturating_add(1)),
            &mut out,
        )?;
        ensure!(
            copied == entry.size,
            "Expanded file size differs from package metadata"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&output, fs::Permissions::from_mode(entry.mode & 0o777))?;
        }
    }

    let mut created_links = Vec::new();
    let link_result = (|| -> Result<()> {
        for (path, target) in &links {
            ensure!(!cancelled.load(Ordering::Relaxed), "Cancelled");
            let output = root.join(path);
            ensure_no_symlink_ancestors(&output)?;
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &output)?;
            #[cfg(not(unix))]
            bail!("Symbolic-link app packages are not supported on this platform");
            created_links.push(output);
        }

        // Validate only after every deferred link exists, allowing normal
        // framework chains such as Resources -> Versions/Current/Resources.
        let mut directory_edges: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for output in &created_links {
            ensure!(!cancelled.load(Ordering::Relaxed), "Cancelled");
            let resolved = output
                .canonicalize()
                .context("Package contains a dangling or cyclic symbolic link")?;
            ensure!(
                resolved.starts_with(&root),
                "Package symbolic link points outside the extraction root"
            );
            if resolved.is_dir() {
                directory_edges
                    .entry(
                        output
                            .parent()
                            .context("Invalid symbolic-link parent")?
                            .to_path_buf(),
                    )
                    .or_default()
                    .push(resolved);
            }
        }

        if !created_links.is_empty() {
            // Directory backlinks can form traversal cycles even when each
            // individual symlink canonicalizes. Check the directory graph too.
            let created: HashSet<_> = created_links.iter().cloned().collect();
            let mut pending = vec![root.clone()];
            while let Some(directory) = pending.pop() {
                ensure!(!cancelled.load(Ordering::Relaxed), "Cancelled");
                directory_edges.entry(directory.clone()).or_default();
                for item in fs::read_dir(&directory)? {
                    let item = item?;
                    let path = item.path();
                    let kind = item.file_type()?;
                    if kind.is_symlink() {
                        ensure!(
                            created.contains(&path),
                            "Extraction destination contains a pre-existing symbolic link"
                        );
                    } else if kind.is_dir() {
                        directory_edges
                            .entry(directory.clone())
                            .or_default()
                            .push(path.clone());
                        pending.push(path);
                    }
                }
            }
            let mut state = HashMap::new();
            for start in directory_edges.keys() {
                let mut pending = vec![(start.clone(), false)];
                while let Some((node, leaving)) = pending.pop() {
                    ensure!(!cancelled.load(Ordering::Relaxed), "Cancelled");
                    if leaving {
                        state.insert(node, 2u8);
                        continue;
                    }
                    match state.get(&node) {
                        Some(2) => continue,
                        Some(1) => bail!("Package symbolic links create a directory cycle"),
                        _ => {}
                    }
                    state.insert(node.clone(), 1);
                    pending.push((node.clone(), true));
                    if let Some(children) = directory_edges.get(&node) {
                        for child in children {
                            pending.push((child.clone(), false));
                        }
                    }
                }
            }
        }
        Ok(())
    })();
    if link_result.is_err() {
        for output in created_links.iter().rev() {
            let _ = fs::remove_file(output);
        }
    }
    link_result
}

pub fn verify_bundle(path: &Path, app: &App, release: Option<&Release>) -> Result<()> {
    ensure!(
        bundle_value(path, "CFBundleIdentifier")? == app.bundle_id,
        "Package is not the expected {} application",
        app.name
    );
    let executable = bundle_value(path, "CFBundleExecutable")?;
    ensure!(
        !executable.contains(['/', '\\']) && !executable.starts_with('.'),
        "Invalid app executable"
    );
    ensure!(
        path.join("Contents/MacOS").join(executable).is_file(),
        "App executable is missing"
    );
    crate::catalog::ensure_macos_version(
        &bundle_value(path, "LSMinimumSystemVersion").unwrap_or_else(|_| app.minimum_macos.into()),
    )?;
    if let Some(release) = release {
        ensure!(
            crate::catalog::normalized_version(&bundle_value(path, "CFBundleShortVersionString")?)
                == crate::catalog::normalized_version(&release.version),
            "App version differs from the catalog"
        );
    }
    if let Some(release) = release {
        ensure!(
            toolbox_open_version(path) >= release.toolbox_open_version,
            "App does not support the Toolbox opening behavior promised by the catalog"
        );
    }
    let mut c = Command::new("/usr/bin/codesign");
    c.args(["--verify", "--deep", "--strict"]).arg(path);
    run(c, "Verify application signature")?;
    if let Some(release) = release.filter(|r| r.is_remote()) {
        let team = release
            .publisher_team
            .as_deref()
            .context("Missing publisher identity")?;
        let requirement =
            format!("anchor apple generic and certificate leaf[subject.OU] = \"{team}\"");
        let mut c = Command::new("/usr/bin/codesign");
        c.args(["--verify", "--strict", "-R", &requirement])
            .arg(path);
        run(c, "Verify Ploeg publisher")?;
        let mut c = Command::new("/usr/sbin/spctl");
        c.args(["--assess", "--type", "execute"]).arg(path);
        run(c, "Check macOS distribution approval")?;
    }
    Ok(())
}

fn download(
    release: &Release,
    output: &Path,
    cancelled: &AtomicBool,
    progress: &impl Fn(Progress),
) -> Result<()> {
    let mut reader: Box<dyn Read>;
    let size;
    if release.is_remote() {
        let client = reqwest::blocking::Client::builder()
            .https_only(true)
            .timeout(Duration::from_secs(900))
            .connect_timeout(Duration::from_secs(20))
            .user_agent("PloegToolbox/0.1")
            .build()?;
        let response = client.get(&release.source).send()?.error_for_status()?;
        size = response.content_length().unwrap_or(0);
        reader = Box::new(response);
    } else {
        let input = fs::File::open(release.local_source()?)
            .context("The local app package could not be opened")?;
        size = input.metadata()?.len();
        reader = Box::new(input);
    }
    let mut target = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let mut received = 0u64;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        ensure!(!cancelled.load(Ordering::Relaxed), "Cancelled");
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        received += n as u64;
        ensure!(
            received <= 8 * 1024 * 1024 * 1024,
            "Download exceeds the 8 GB limit"
        );
        target.write_all(&buffer[..n])?;
        progress(Progress::Downloading(received, size));
    }
    target.sync_all()?;
    Ok(())
}

fn activate(store: &Store, app: &App, staged: &Path) -> Result<PathBuf> {
    ensure_no_symlink_ancestors(&store.apps)?;
    let destination = store.managed_path(app.name)?;
    ensure!(
        is_inside(&destination, &store.apps),
        "Destination must be managed by PloegToolbox"
    );
    ensure!(
        !store.apps.is_symlink() && !destination.is_symlink(),
        "Managed installation paths cannot be symlinks"
    );
    ensure!(
        !is_running(&destination),
        "Quit {} before installing this update",
        app.name
    );
    fs::create_dir_all(&store.apps)?;
    fs::create_dir_all(store.root.join("backups"))?;
    let backup = store.root.join("backups").join(format!("{}.app", app.id));
    ensure!(
        !backup.exists(),
        "An earlier update needs recovery; restart PloegToolbox"
    );
    let mut journal = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(store.recovery_path())?;
    journal.write_all(&serde_json::to_vec(&Journal { app: app.id.into() })?)?;
    journal.sync_all()?;
    if destination.exists() {
        fs::rename(&destination, &backup).context("Could not stage the previous installation")?;
    }
    // Staging is moved onto the destination volume before activation so the final rename is atomic.
    let incoming = store.apps.join(format!(".{}-incoming.app", app.id));
    let result = (|| -> Result<()> {
        ensure!(
            !incoming.exists() && !incoming.is_symlink(),
            "An earlier install needs recovery; restart PloegToolbox"
        );
        let mut c = Command::new("/usr/bin/ditto");
        c.arg(staged).arg(&incoming);
        run(c, "Stage verified app")?;
        verify_bundle(&incoming, app, None)?;
        fs::rename(&incoming, &destination)?;
        Ok(())
    })();
    if let Err(err) = result {
        if incoming.exists() {
            let _ = fs::remove_dir_all(&incoming);
        }
        if backup.exists() {
            fs::rename(&backup, &destination).context("Rollback needs recovery. Restart PloegToolbox; the previous app is retained in its backup folder.")?;
        }
        let _ = fs::remove_file(store.recovery_path());
        return Err(err);
    }
    if backup.exists() {
        fs::remove_dir_all(&backup)?;
    }
    fs::remove_file(store.recovery_path())?;
    Ok(destination)
}

fn operation_lock(store: &Store) -> Result<fs::File> {
    ensure_no_symlink_ancestors(&store.root)?;
    ensure_no_symlink_ancestors(&store.apps)?;
    ensure_no_symlink_ancestors(&store.root.join("backups"))?;
    ensure_no_symlink_ancestors(&store.root.join("staging"))?;
    fs::create_dir_all(&store.root)?;
    let path = store.root.join("install.lock");
    ensure!(!path.is_symlink(), "Install lock cannot be a symlink");
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("Another PloegToolbox operation is in progress")?;
    Ok(lock)
}

pub fn recover(store: &Store) -> Result<()> {
    if !store.recovery_path().exists() {
        return Ok(());
    }
    let _lock = operation_lock(store)?;
    recover_locked(store)
}

fn recover_locked(store: &Store) -> Result<()> {
    ensure_no_symlink_ancestors(&store.apps)?;
    let path = store.recovery_path();
    if !path.exists() {
        return Ok(());
    }
    let journal: Journal = serde_json::from_slice(&fs::read(&path)?)?;
    let app = crate::catalog::APPS
        .iter()
        .find(|a| a.id == journal.app)
        .context("Unknown app in recovery record")?;
    let destination = store.managed_path(app.name)?;
    let backup = store.root.join("backups").join(format!("{}.app", app.id));
    ensure!(
        !destination.is_symlink() && !backup.is_symlink(),
        "Recovery paths cannot be symlinks"
    );
    ensure!(
        !is_running(&destination),
        "Quit {} to finish installation recovery",
        app.name
    );
    if backup.exists() {
        if destination.exists() {
            fs::remove_dir_all(&destination)?;
        }
        fs::rename(backup, destination)?;
    }
    let incoming = store.apps.join(format!(".{}-incoming.app", app.id));
    if incoming.exists() {
        fs::remove_dir_all(incoming)?;
    }
    fs::remove_file(path)?;
    Ok(())
}

pub fn install(
    store: &Store,
    app: &App,
    release: &Release,
    cancelled: Arc<AtomicBool>,
    progress: impl Fn(Progress),
) -> Result<PathBuf> {
    release.validate()?;
    ensure!(release.app == app.id, "Release belongs to another app");
    let _lock = operation_lock(store)?;
    recover_locked(store)?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let staging = store
        .root
        .join("staging")
        .join(format!("{}-{nonce}", app.id));
    fs::create_dir_all(&staging)?;
    let result = (|| -> Result<PathBuf> {
        let archive = staging.join("package.zip");
        download(release, &archive, &cancelled, &progress)?;
        progress(Progress::Verifying);
        verify_hash(&archive, &release.sha256)?;
        let extracted = staging.join("unpacked");
        fs::create_dir(&extracted)?;
        extract_zip(&archive, &extracted, &cancelled)?;
        let bundle = extracted.join(format!("{}.app", app.name));
        ensure!(
            bundle.is_dir(),
            "Package must contain {}.app at its root",
            app.name
        );
        verify_bundle(&bundle, app, Some(release))?;
        ensure!(!cancelled.load(Ordering::Relaxed), "Cancelled");
        progress(Progress::Installing);
        activate(store, app, &bundle)
    })();
    let _ = fs::remove_dir_all(staging);
    result
}

pub fn remove(store: &Store, app: &App) -> Result<()> {
    let _lock = operation_lock(store)?;
    ensure!(
        !store.recovery_path().exists(),
        "An earlier installation needs recovery; restart PloegToolbox"
    );
    let path = store.managed_path(app.name)?;
    ensure!(
        is_inside(&path, &store.apps) && !path.is_symlink(),
        "Only managed app bundles can be removed"
    );
    ensure!(
        !is_running(&path),
        "Quit {} before removing its managed copy",
        app.name
    );
    if !path.exists() {
        bail!("Managed copy is already absent");
    }
    verify_bundle(&path, app, None)?;
    fs::remove_dir_all(path)?;
    Ok(())
}
