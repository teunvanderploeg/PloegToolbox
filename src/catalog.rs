use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct App {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub description: &'static str,
    pub bundle_id: &'static str,
    pub minimum_macos: &'static str,
    pub setup: &'static str,
    pub privacy: &'static str,
    pub icon: &'static [u8],
}

pub const APPS: [App; 3] = [
    App {
        id: "sound",
        name: "SoundPloeg",
        summary: "Read text aloud",
        description: "Read clipboard text in English or Dutch, manage a reading queue and replay recent speech.",
        bundle_id: "com.soundploeg.app",
        minimum_macos: "13",
        setup: "Choose a voice and install its optional voice pack in SoundPloeg.",
        privacy: "Speech runs locally after setup.",
        icon: include_bytes!("../assets/icons/sound.png"),
    },
    App {
        id: "voice",
        name: "VoicePloeg",
        summary: "Turn speech into text",
        description: "Dictate into your apps with local transcription, optional live phrase insertion and transcript history.",
        bundle_id: "com.voiceploeg.app",
        minimum_macos: "14",
        setup: "Allow microphone and Accessibility access in VoicePloeg, then choose a local model.",
        privacy: "Transcription runs locally after setup.",
        icon: include_bytes!("../assets/icons/voice.png"),
    },
    App {
        id: "shot",
        name: "PloegShot",
        summary: "Capture and annotate",
        description: "Capture an area or display, add annotations or permanent redaction, then copy or save the image.",
        bundle_id: "nl.ploegshot.PloegShot",
        minimum_macos: "13",
        setup: "Allow Screen Recording when PloegShot asks to capture your screen.",
        privacy: "Screenshots and edits stay on your Mac.",
        icon: include_bytes!("../assets/icons/shot.png"),
    },
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Release {
    pub app: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub source: String,
    pub sha256: String,
    #[serde(default)]
    pub toolbox_open_version: u32,
    #[serde(default)]
    pub publisher_team: Option<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(skip)]
    pub catalog_dir: PathBuf,
}

impl Release {
    pub fn is_remote(&self) -> bool {
        self.source.starts_with("https://")
    }

    pub fn compatible(&self) -> bool {
        self.os == std::env::consts::OS
            && (self.arch == std::env::consts::ARCH || self.arch == "universal")
    }

    pub fn local_source(&self) -> Result<PathBuf> {
        ensure!(
            !self.source.contains("://"),
            "Only HTTPS downloads or local package paths are supported"
        );
        let path = Path::new(&self.source);
        Ok(if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.catalog_dir.join(path)
        })
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            APPS.iter().any(|a| a.id == self.app),
            "Unknown Ploeg app in catalog"
        );
        semver::Version::parse(&normalized_version(&self.version))
            .context("Invalid release version")?;
        ensure!(
            self.sha256.len() == 64 && self.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "Each package needs a SHA-256 checksum"
        );
        ensure!(
            self.compatible(),
            "This package does not support this OS and processor"
        );
        // Only Mac app bundles have an installer in this foundation release.
        ensure!(
            self.os == "macos",
            "App installation is currently available on macOS"
        );
        let app = APPS
            .iter()
            .find(|a| a.id == self.app)
            .context("Unknown app")?;
        ensure_macos_version(app.minimum_macos)?;
        if self.is_remote() {
            let team = self.publisher_team.as_deref().unwrap_or("");
            ensure!(
                team.len() == 10 && team.bytes().all(|b| b.is_ascii_alphanumeric()),
                "A verified Ploeg publisher team is required for a remote package"
            );
            let trusted = option_env!("PLOEG_PUBLISHER_TEAM").unwrap_or("");
            ensure!(
                !trusted.is_empty() && team == trusted,
                "Remote downloads are not enabled for this publisher. Use a trusted local build catalog until Ploeg signing is configured."
            );
        } else {
            self.local_source()?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    schema: u32,
    releases: Vec<Release>,
}

pub fn load(path: &Path) -> Result<Vec<Release>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("Cannot read catalog {}", path.display()))?;
    ensure!(file.metadata()?.len() <= 1_048_576, "Catalog is too large");
    let mut data = Vec::new();
    file.take(1_048_577).read_to_end(&mut data)?;
    ensure!(data.len() <= 1_048_576, "Catalog is too large");
    let mut catalog: Catalog =
        serde_json::from_slice(&data).context("Invalid Ploeg release catalog")?;
    ensure!(catalog.schema == 1, "Unsupported catalog format");
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    for release in &mut catalog.releases {
        release.catalog_dir = base.clone();
    }
    Ok(catalog.releases)
}

pub fn normalized_version(value: &str) -> String {
    let raw = value.trim_start_matches('v');
    match raw.split('.').count() {
        1 => format!("{raw}.0.0"),
        2 => format!("{raw}.0"),
        _ => raw.to_owned(),
    }
}

pub fn newer(a: &str, b: &str) -> bool {
    semver::Version::parse(&normalized_version(a))
        .ok()
        .zip(semver::Version::parse(&normalized_version(b)).ok())
        .is_some_and(|(a, b)| a > b)
}

pub fn ensure_macos_version(minimum: &str) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    static CURRENT: std::sync::OnceLock<std::result::Result<semver::Version, String>> =
        std::sync::OnceLock::new();
    let current = CURRENT
        .get_or_init(|| {
            let output = std::process::Command::new("/usr/bin/sw_vers")
                .arg("-productVersion")
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                return Err("Cannot determine macOS compatibility".into());
            }
            semver::Version::parse(&normalized_version(
                String::from_utf8_lossy(&output.stdout).trim(),
            ))
            .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!(e.clone()))?;
    let required = semver::Version::parse(&normalized_version(minimum))?;
    ensure!(
        *current >= required,
        "This app requires macOS {minimum} or later"
    );
    Ok(())
}
