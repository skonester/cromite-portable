//! The installed-version marker (`app/.cromite-version`).
//!
//! Chromium builds carry no machine-readable release tag, so — like Nomad's
//! `.nomad-version` — the updater records one itself. The marker is an
//! assertion, not a hint: it is written into the stage only after the package
//! was verified and the staged tree validated, and any copy shipped inside the
//! upstream archive is discarded.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::github::VersionInfo;

pub const VERSION_MARKER: &str = ".cromite-version";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledVersion {
    pub tag: String,
    pub chromium_version: String,
}

impl From<&VersionInfo> for InstalledVersion {
    fn from(info: &VersionInfo) -> Self {
        Self {
            tag: info.tag.clone(),
            chromium_version: info.chromium_version.clone(),
        }
    }
}

/// What is currently in `app/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// Installed by this updater; the exact release tag is known.
    Marked(InstalledVersion),
    /// Installed some other way (e.g. the old PowerShell script); only the
    /// Chromium version could be detected from the build's layout.
    Detected(String),
    /// A `chrome.exe` exists but its version could not be determined.
    Unknown,
    /// No browser installed.
    Missing,
}

impl std::fmt::Display for Installed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Marked(v) => write!(f, "{}", v.tag),
            Self::Detected(v) => write!(f, "{v} (detected; no updater marker)"),
            Self::Unknown => write!(f, "unknown version"),
            Self::Missing => write!(f, "not installed"),
        }
    }
}

/// `153.0.8010.37-11507ac1…` → `153.0.8010.37 (11507ac)` for display.
#[must_use]
pub fn short_tag(tag: &str) -> String {
    match tag.split_once('-') {
        Some((version, commit)) => format!("{version} ({})", commit.get(..7).unwrap_or(commit)),
        None => tag.to_owned(),
    }
}

pub fn write_marker(install_dir: &Path, version: &InstalledVersion) -> Result<()> {
    let json = serde_json::to_string_pretty(version).map_err(std::io::Error::other)?;
    std::fs::write(install_dir.join(VERSION_MARKER), json)?;
    Ok(())
}

pub fn installed(install_dir: &Path) -> Installed {
    if let Some(v) = std::fs::read_to_string(install_dir.join(VERSION_MARKER))
        .ok()
        .and_then(|raw| serde_json::from_str::<InstalledVersion>(&raw).ok())
    {
        return Installed::Marked(v);
    }
    if !install_dir.join("chrome.exe").is_file() {
        return Installed::Missing;
    }
    detect_chromium_version(install_dir).map_or(Installed::Unknown, Installed::Detected)
}

/// Chromium Windows builds ship `<version>.manifest` (and, in installer
/// layouts, a `<version>/` directory) beside `chrome.exe`. The highest
/// four-part numeric name found is taken as the installed version.
fn detect_chromium_version(install_dir: &Path) -> Option<String> {
    std::fs::read_dir(install_dir)
        .ok()?
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".manifest").unwrap_or(&name).to_owned();
            let parts: Vec<u32> = stem
                .split('.')
                .map(|p| p.parse().ok())
                .collect::<Option<_>>()?;
            (parts.len() == 4).then_some((parts, stem))
        })
        .max()
        .map(|(_, stem)| stem)
}

/// Whether `latest` should replace what is installed.
#[must_use]
pub fn needs_update(installed: &Installed, latest: &VersionInfo) -> bool {
    match installed {
        Installed::Marked(v) => v.tag != latest.tag,
        Installed::Detected(v) => *v != latest.chromium_version,
        Installed::Unknown | Installed::Missing => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latest() -> VersionInfo {
        VersionInfo {
            tag: "153.0.8010.37-abc".to_owned(),
            chromium_version: "153.0.8010.37".to_owned(),
            download_url: String::new(),
            sha256: String::new(),
            size: 0,
            published: None,
        }
    }

    #[test]
    fn short_tag_abbreviates_the_commit() {
        assert_eq!(
            short_tag("153.0.8010.37-11507ac1061b5ea227806f5e84db5a57df6ccf6a"),
            "153.0.8010.37 (11507ac)"
        );
        assert_eq!(short_tag("153.0.8010.37"), "153.0.8010.37");
    }

    #[test]
    fn marker_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let v = InstalledVersion::from(&latest());
        write_marker(dir.path(), &v).unwrap();
        assert_eq!(installed(dir.path()), Installed::Marked(v));
    }

    #[test]
    fn missing_when_nothing_installed() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(installed(dir.path()), Installed::Missing);
    }

    #[test]
    fn detects_version_from_manifest_without_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chrome.exe"), b"x").unwrap();
        std::fs::write(dir.path().join("147.0.7727.56.manifest"), b"x").unwrap();
        std::fs::write(dir.path().join("chrome.dll"), b"x").unwrap();
        assert_eq!(
            installed(dir.path()),
            Installed::Detected("147.0.7727.56".to_owned())
        );
    }

    #[test]
    fn unknown_when_version_undetectable() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chrome.exe"), b"x").unwrap();
        assert_eq!(installed(dir.path()), Installed::Unknown);
    }

    #[test]
    fn needs_update_rules() {
        let l = latest();
        assert!(needs_update(&Installed::Missing, &l));
        assert!(needs_update(&Installed::Unknown, &l));
        assert!(needs_update(
            &Installed::Detected("147.0.7727.56".to_owned()),
            &l
        ));
        assert!(!needs_update(
            &Installed::Detected("153.0.8010.37".to_owned()),
            &l
        ));
        assert!(!needs_update(
            &Installed::Marked(InstalledVersion::from(&l)),
            &l
        ));
        let rebuilt = InstalledVersion {
            tag: "153.0.8010.37-def".to_owned(),
            chromium_version: "153.0.8010.37".to_owned(),
        };
        assert!(
            needs_update(&Installed::Marked(rebuilt), &l),
            "same Chromium, new Cromite commit"
        );
    }
}
