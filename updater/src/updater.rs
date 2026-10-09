//! The single download → verify → extract → finalize → swap sequence.
//!
//! Mirrors Nomad Launcher `core/src/updater.rs` (`download_and_install` /
//! `finalize_install`). Any failure before the swap leaves the working `app/`
//! untouched; the profile lives in `data/`, outside the swapped directory.

use std::path::Path;

use crate::error::Result;
use crate::github::VersionInfo;
use crate::{download, extract, install, version};

/// A coarse step, reported so the window can show per-step status text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Downloading,
    Verifying,
    Extracting,
    Installing,
}

pub fn download_and_install(
    client: &reqwest::blocking::Client,
    app: &Path,
    latest: &VersionInfo,
    mut on_step: impl FnMut(Step),
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<()> {
    let stage = install::stage_dir(app);
    let backup = install::backup_dir(app);
    let downloads = install::download_dir(app);
    std::fs::create_dir_all(&downloads)
        .map_err(install::io_context("creating the download folder"))?;
    std::fs::create_dir_all(&stage).map_err(install::io_context("creating the staging folder"))?;
    let package = downloads.join("chrome-win.zip");

    // A package left by an earlier failed install is reused only if it still
    // matches this release's digest; otherwise it is downloaded again.
    on_step(Step::Verifying);
    if download::verify_package(&package, &latest.sha256, latest.size).is_err() {
        on_step(Step::Downloading);
        download::download(client, &latest.download_url, &package, on_progress)?;
        on_step(Step::Verifying);
        download::verify_package(&package, &latest.sha256, latest.size)?;
    }

    on_step(Step::Extracting);
    extract::extract_zip(&package, &stage)?;

    on_step(Step::Installing);
    finalize_install(app, &stage, &backup, latest)?;

    // Cleanup only: a scanner still holding the zip must not fail the update.
    if let Err(e) = install::remove_dir_all(&downloads) {
        eprintln!("warning: could not remove {}: {e}", downloads.display());
    }
    Ok(())
}

/// Validates the staged build, writes the version marker into it, and swaps
/// it into place. Nothing outside the stage changes until the swap.
fn finalize_install(app: &Path, stage: &Path, backup: &Path, latest: &VersionInfo) -> Result<()> {
    install::validate_staged_install(stage)?;
    version::write_marker(stage, &latest.into())?;
    install::ensure_not_running(app)?;
    install::atomic_swap(app, stage, backup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn incompatible_stage_preserves_the_working_install() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        let stage = install::stage_dir(&app);
        let backup = install::backup_dir(&app);
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("chrome.exe"), b"old").unwrap();
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("chrome.exe"), b"new").unwrap(); // no chrome.dll etc.

        let latest = VersionInfo {
            tag: "153.0.8010.37-abc".to_owned(),
            chromium_version: "153.0.8010.37".to_owned(),
            download_url: String::new(),
            sha256: String::new(),
            size: 0,
            published: None,
        };
        let err = finalize_install(&app, &stage, &backup, &latest)
            .expect_err("an incomplete build must abort before the swap");
        assert!(matches!(err, Error::Compatibility(_)));
        assert_eq!(std::fs::read(app.join("chrome.exe")).unwrap(), b"old");
        assert!(!backup.exists(), "the swap must not have started");
        assert!(
            !stage.join(version::VERSION_MARKER).exists(),
            "no success marker for a failed validation"
        );
    }
}
