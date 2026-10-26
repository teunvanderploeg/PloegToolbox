//! Install/verify/remove a local build catalog in a new isolated test directory.
use anyhow::{Context, Result, ensure};
use ploegtoolbox::{
    catalog::{self, APPS},
    install,
    store::Store,
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 3,
        "Usage: verify_local_packages <catalog.json> <new absolute scratch directory>"
    );
    let catalog = PathBuf::from(&args[1]).canonicalize()?;
    let scratch = PathBuf::from(&args[2]);
    ensure!(
        scratch.is_absolute() && !scratch.exists(),
        "Scratch directory must be absolute and must not already exist"
    );
    let releases = catalog::load(&catalog)?;
    std::fs::create_dir(&scratch)?;
    let store = Store {
        root: scratch.join("state"),
        apps: scratch.join("apps"),
    };
    for release in releases.iter().filter(|r| r.compatible()) {
        ensure!(
            !release.is_remote(),
            "This diagnostic accepts only local build packages"
        );
        let app = APPS
            .iter()
            .find(|a| a.id == release.app)
            .context("Unknown app")?;
        println!("Checking {} {}", app.name, release.version);
        let path = install::install(
            &store,
            app,
            release,
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )?;
        install::verify_bundle(&path, app, Some(release))?;
        ensure!(
            install::discover(&store, app).is_some_and(|i| i.managed),
            "Installed app was not discovered"
        );
        install::remove(&store, app)?;
        ensure!(!path.exists(), "Managed app was not removed");
        println!(
            "Verified install, signature, discovery and removal for {}",
            app.name
        );
    }
    std::fs::remove_dir_all(scratch)?;
    Ok(())
}
