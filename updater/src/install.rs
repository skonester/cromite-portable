//! Atomic browser install: stage → swap → backup.
//!
//! Adapted from Nomad Launcher `core/src/install.rs` (MIT, see
//! `THIRD_PARTY_NOTICES.md`).
//!
//! Extraction goes into an `app.stage` sibling of `app`. Once the stage is
//! fully verified and prepared, a pair of renames moves the old install to
//! `app.backup` and the stage to `app`. A crash at any point before the swap
//! leaves `app` intact; a crash between the two renames is detected and
//! recovered by [`recover_staging`] on the next run.
//!
//! Every rename and delete goes through [`retry_transient`]: antivirus
//! scanners and the Windows Search indexer briefly open freshly written files
//! (a 200 MB zip, 465 extracted files) without delete sharing, which makes an
//! immediate rename or delete fail with a sharing violation.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{Error, Result};

fn sibling(install_dir: &Path, suffix: &str) -> PathBuf {
    let mut s: OsString = install_dir.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Returns the staging directory path (`<install_dir>.stage`).
#[must_use]
pub fn stage_dir(install_dir: &Path) -> PathBuf {
    sibling(install_dir, ".stage")
}

/// Returns the backup directory path (`<install_dir>.backup`).
#[must_use]
pub fn backup_dir(install_dir: &Path) -> PathBuf {
    sibling(install_dir, ".backup")
}

/// Returns the download directory path (`<install_dir>.download`). Kept
/// apart from the stage so the package never ends up inside `app/`, and so a
/// verified download survives a failed install and can be reused on retry.
#[must_use]
pub fn download_dir(install_dir: &Path) -> PathBuf {
    sibling(install_dir, ".download")
}

/// Backoff for [`retry_transient`]: ~15 s in total.
const RETRY_DELAYS_MS: &[u64] = &[100, 200, 400, 800, 1000, 1500, 2000, 3000, 3000, 3000];

/// `ERROR_ACCESS_DENIED` (5) or `ERROR_SHARING_VIOLATION` (32): what Windows
/// reports while another process holds a handle without delete sharing.
fn is_transient(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(5 | 32))
}

/// Runs `op`, retrying with backoff while it fails with a transient
/// sharing/access error. Other errors are returned immediately.
pub fn retry_transient<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    for &delay in RETRY_DELAYS_MS {
        match op() {
            Err(e) if is_transient(&e) => std::thread::sleep(Duration::from_millis(delay)),
            result => return result,
        }
    }
    op()
}

/// Wraps an I/O error with what was being done, so the window says which
/// step failed instead of a bare OS message.
pub fn io_context(what: impl std::fmt::Display) -> impl FnOnce(io::Error) -> Error {
    move |e| Error::Io(io::Error::new(e.kind(), format!("{what}: {e}")))
}

fn rename(from: &Path, to: &Path) -> io::Result<()> {
    retry_transient(|| std::fs::rename(from, to))
}

/// Removes a directory tree, retrying transient failures. Already-gone is OK.
pub fn remove_dir_all(dir: &Path) -> io::Result<()> {
    retry_transient(|| match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    })
}

/// Recovers from a previously interrupted install and removes leftover staging
/// directories. Call once per run before the update check.
///
/// - **Mid-swap crash**: `install_dir` was renamed to backup before the stage
///   was moved into place. Restores the backup so Cromite stays launchable.
/// - **Stale stage**: a crash during download/extraction left `.stage` behind.
/// - **Orphaned backup**: the swap completed but backup deletion failed.
pub fn recover_staging(install_dir: &Path) {
    let stage = stage_dir(install_dir);
    let backup = backup_dir(install_dir);

    if !install_dir.exists() && backup.exists() {
        eprintln!("warning: app folder missing with backup present; restoring from backup");
        if let Err(e) = rename(&backup, install_dir) {
            eprintln!("error: could not restore install backup: {e}");
        }
    }

    if stage.exists() {
        if let Err(e) = remove_dir_all(&stage) {
            eprintln!(
                "warning: could not remove stale stage {}: {e}",
                stage.display()
            );
        }
    }

    if backup.exists() && install_dir.exists() {
        if let Err(e) = remove_dir_all(&backup) {
            eprintln!(
                "warning: could not remove stale backup {}: {e}",
                backup.display()
            );
        }
    }
}

/// Moves `stage` into place as `install_dir` via a rename-based swap.
///
/// 1. Remove any leftover `backup` from a previous run.
/// 2. Rename `install_dir` → `backup` (skipped on a fresh install).
/// 3. Rename `stage` → `install_dir`. On failure, restore `backup`.
/// 4. Remove `backup` (best-effort; cleaned by [`recover_staging`] otherwise).
///
/// All three directories are siblings, so each rename is a single metadata
/// operation on the same volume.
pub fn atomic_swap(install_dir: &Path, stage: &Path, backup: &Path) -> Result<()> {
    if backup.exists() {
        remove_dir_all(backup).map_err(io_context("removing the old backup folder"))?;
    }

    if install_dir.exists() {
        // Windows refuses to rename a directory with open files in it. If that
        // persists past the retries, Cromite (or something inside app/) is
        // still running; nothing has changed yet.
        rename(install_dir, backup).map_err(|e| {
            if is_transient(&e) {
                Error::InUse
            } else {
                io_context("moving the current build aside")(e)
            }
        })?;
    }

    if let Err(e) = rename(stage, install_dir) {
        if backup.exists() {
            match rename(backup, install_dir) {
                Ok(()) => {
                    eprintln!("warning: install swap failed; rolled back to previous install")
                }
                Err(re) => eprintln!(
                    "error: swap failed AND rollback failed ({re}); the previous install is at {}",
                    backup.display()
                ),
            }
        }
        return Err(io_context("moving the new build into place")(e));
    }

    if backup.exists() {
        if let Err(e) = remove_dir_all(backup) {
            eprintln!(
                "warning: could not remove install backup ({e}); will be cleaned on next run"
            );
        }
    }

    Ok(())
}

/// Files a working Cromite Windows build must contain. Checked against the
/// staged tree before it may replace the working install.
const REQUIRED_FILES: &[&str] = &[
    "chrome.exe",
    "chrome.dll",
    "chrome_elf.dll",
    "resources.pak",
    "icudtl.dat",
    "chrome_100_percent.pak",
    "chrome_200_percent.pak",
];

/// The compatibility gate on the *staged artifact* (Nomad's
/// `validate_chromium_stage`, minus logo branding which Cromite does not do).
/// Also discards any version marker shipped inside the upstream archive, so
/// only a marker written by us after verification can exist.
pub fn validate_staged_install(stage: &Path) -> Result<()> {
    let marker = stage.join(crate::version::VERSION_MARKER);
    if marker.exists() {
        retry_transient(|| std::fs::remove_file(&marker))?;
    }
    for name in REQUIRED_FILES {
        if !std::fs::metadata(stage.join(name)).is_ok_and(|m| m.is_file() && m.len() > 0) {
            return Err(Error::Compatibility(format!(
                "required file `{name}` is missing or empty in the new build; the working browser was not replaced"
            )));
        }
    }
    Ok(())
}

/// Advisory pre-check: refuses early if `chrome.exe` in `install_dir` is
/// currently running. Windows denies write access to a mapped executable
/// image with a sharing violation (os error 32). Opening for write without
/// truncation leaves the file untouched.
pub fn ensure_not_running(install_dir: &Path) -> Result<()> {
    let exe = install_dir.join("chrome.exe");
    if !exe.exists() {
        return Ok(());
    }
    match std::fs::OpenOptions::new().write(true).open(&exe) {
        Err(e) if e.raw_os_error() == Some(32) => Err(Error::InUse),
        // Anything else (read-only attribute, ACLs) is not proof Cromite is
        // running; the swap itself is the authoritative check.
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_transient_returns_other_errors_immediately() {
        let mut calls = 0;
        let result: io::Result<()> = retry_transient(|| {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::NotFound))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn retry_transient_recovers_from_a_sharing_violation() {
        let mut calls = 0;
        let result = retry_transient(|| {
            calls += 1;
            if calls < 3 {
                Err(io::Error::from_raw_os_error(32))
            } else {
                Ok(calls)
            }
        });
        assert_eq!(result.unwrap(), 3);
    }

    /// The real failure: another process holds the file open without delete
    /// sharing (as Defender / the search indexer do), then lets go.
    #[cfg(windows)]
    #[test]
    fn rename_waits_out_a_handle_held_by_a_scanner() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("chrome-win.zip");
        let to = dir.path().join("moved.zip");
        std::fs::write(&from, b"pkg").unwrap();

        // FILE_SHARE_READ only: rename/delete are refused while this is open.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1)
            .open(&from)
            .unwrap();
        assert_eq!(
            std::fs::rename(&from, &to).unwrap_err().raw_os_error(),
            Some(32),
            "precondition: an unretried rename fails like the user's run did"
        );
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            drop(held);
        });

        rename(&from, &to).expect("the retry must outlast the scanner's handle");
        releaser.join().unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"pkg");
    }

    #[test]
    fn stage_and_backup_are_siblings_of_install_dir() {
        let install = Path::new("C:/portable/app");
        assert_eq!(stage_dir(install), Path::new("C:/portable/app.stage"));
        assert_eq!(backup_dir(install), Path::new("C:/portable/app.backup"));
    }

    #[test]
    fn atomic_swap_rolls_back_when_the_stage_rename_fails() {
        let dir = tempfile::tempdir().unwrap();
        let install = dir.path().join("app");
        let stage = stage_dir(&install);
        let backup = backup_dir(&install);
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("chrome.exe"), b"old install").unwrap();

        let err = atomic_swap(&install, &stage, &backup)
            .expect_err("a missing stage directory must fail the swap");
        assert!(matches!(err, Error::Io(_)), "got {err:?}");
        assert_eq!(
            std::fs::read(install.join("chrome.exe")).unwrap(),
            b"old install"
        );
        assert!(!backup.exists());
    }

    #[test]
    fn atomic_swap_fresh_install() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("app");
        let stage = stage_dir(&install);
        let backup = backup_dir(&install);
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("chrome.exe"), b"NEW").unwrap();

        atomic_swap(&install, &stage, &backup).unwrap();

        assert_eq!(std::fs::read(install.join("chrome.exe")).unwrap(), b"NEW");
        assert!(!stage.exists());
        assert!(!backup.exists());
    }

    #[test]
    fn atomic_swap_replaces_existing_install() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("app");
        let stage = stage_dir(&install);
        let backup = backup_dir(&install);
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("chrome.exe"), b"OLD").unwrap();
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("chrome.exe"), b"NEW").unwrap();

        atomic_swap(&install, &stage, &backup).unwrap();

        assert_eq!(std::fs::read(install.join("chrome.exe")).unwrap(), b"NEW");
        assert!(!stage.exists());
        assert!(!backup.exists());
    }

    #[test]
    fn recover_staging_removes_stale_stage() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("app");
        let stage = stage_dir(&install);
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("partial.exe"), b"PARTIAL").unwrap();

        recover_staging(&install);

        assert!(!stage.exists());
        assert!(install.exists());
    }

    #[test]
    fn recover_staging_restores_install_from_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("app");
        let backup = backup_dir(&install);
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("chrome.exe"), b"PREV").unwrap();

        recover_staging(&install);

        assert_eq!(std::fs::read(install.join("chrome.exe")).unwrap(), b"PREV");
        assert!(!backup.exists());
    }

    #[test]
    fn recover_staging_removes_orphaned_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path().join("app");
        let backup = backup_dir(&install);
        std::fs::create_dir_all(&install).unwrap();
        std::fs::create_dir_all(&backup).unwrap();

        recover_staging(&install);

        assert!(!backup.exists());
        assert!(install.exists());
    }

    fn complete_stage(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        for name in REQUIRED_FILES {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
    }

    #[test]
    fn validate_staged_install_accepts_a_complete_build() {
        let tmp = tempfile::tempdir().unwrap();
        complete_stage(tmp.path());
        validate_staged_install(tmp.path()).unwrap();
    }

    #[test]
    fn validate_staged_install_rejects_missing_or_empty_files() {
        let tmp = tempfile::tempdir().unwrap();
        complete_stage(tmp.path());
        std::fs::write(tmp.path().join("chrome.dll"), b"").unwrap();
        assert!(matches!(
            validate_staged_install(tmp.path()),
            Err(Error::Compatibility(_))
        ));

        complete_stage(tmp.path());
        std::fs::remove_file(tmp.path().join("icudtl.dat")).unwrap();
        assert!(matches!(
            validate_staged_install(tmp.path()),
            Err(Error::Compatibility(_))
        ));
    }

    #[test]
    fn validate_staged_install_discards_an_upstream_supplied_marker() {
        let tmp = tempfile::tempdir().unwrap();
        complete_stage(tmp.path());
        let marker = tmp.path().join(crate::version::VERSION_MARKER);
        std::fs::write(&marker, b"{\"tag\":\"forged\"}").unwrap();

        validate_staged_install(tmp.path()).unwrap();
        assert!(
            !marker.exists(),
            "a marker shipped in the archive must never be trusted"
        );
    }
}
