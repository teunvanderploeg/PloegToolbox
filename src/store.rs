use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub appearance: Appearance,
    pub catalog: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
    pub apps: PathBuf,
}

impl Store {
    pub fn standard() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("nl", "Ploeg", "PloegToolbox")
            .context("Cannot locate application data")?;
        let home = directories::BaseDirs::new().context("Cannot locate home folder")?;
        let root = dirs.data_local_dir().to_path_buf();
        let apps = if cfg!(target_os = "macos") {
            home.home_dir().join("Applications/PloegToolbox")
        } else {
            root.join("apps")
        };
        Ok(Self { root, apps })
    }

    pub fn settings(&self) -> Result<Settings> {
        let path = self.root.join("settings.json");
        if !path.exists() {
            return Ok(Settings::default());
        }
        let data = std::fs::read(path)?;
        serde_json::from_slice(&data)
            .context("Settings could not be read; the file has been preserved")
    }

    pub fn save(&self, settings: &Settings) -> Result<()> {
        ensure_no_symlink_ancestors(&self.root)?;
        std::fs::create_dir_all(&self.root)?;
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let staging = self
            .root
            .join(format!(".settings-{}-{nonce}.tmp", std::process::id()));
        let result = (|| -> Result<()> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staging)?;
            file.write_all(&serde_json::to_vec_pretty(settings)?)?;
            file.sync_all()?;
            std::fs::rename(&staging, self.root.join("settings.json"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(staging);
        }
        result
    }

    pub fn managed_path(&self, name: &str) -> Result<PathBuf> {
        ensure!(
            !name.is_empty() && !name.contains(['/', '\\']) && !name.starts_with('.'),
            "Invalid application name"
        );
        Ok(self.apps.join(format!("{name}.app")))
    }

    pub fn recovery_path(&self) -> PathBuf {
        self.root.join("pending-install.json")
    }
}

pub fn is_inside(path: &Path, root: &Path) -> bool {
    path.parent() == Some(root) && path.extension().is_some_and(|e| e == "app")
}

pub fn ensure_no_symlink_ancestors(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute(),
        "Application directories must be absolute paths"
    );
    ensure!(
        !path.ancestors().any(Path::is_symlink),
        "Application directories cannot contain symlinks"
    );
    ensure!(
        !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir)),
        "Application directories cannot contain parent traversal"
    );
    Ok(())
}
