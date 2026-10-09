//! Zip extraction into the staging directory.
//!
//! Adapted from Nomad Launcher `core/src/extract.rs` (MIT, see
//! `THIRD_PARTY_NOTICES.md`). The NSIS/7-Zip path is dropped: Cromite ships
//! a plain `chrome-win.zip`.

use std::fs::File;
use std::path::{Component, Path};

use crate::error::{Error, Result};

/// Cumulative decompressed-size ceiling. A Cromite build expands to well
/// under 1 GiB; a crafted high-ratio archive past this aborts extraction
/// instead of filling the portable drive.
const MAX_DECOMPRESSED_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Returns the single shared top-level directory across all `names` (e.g.
/// `chrome-win/`), if there is exactly one, so it can be stripped.
fn common_top_dir(names: &[String]) -> Option<String> {
    let mut top: Option<&str> = None;
    for name in names {
        let first = name.split('/').next().filter(|s| !s.is_empty())?;
        match top {
            None => top = Some(first),
            Some(t) if t == first => {}
            Some(_) => return None,
        }
    }
    top.map(|t| format!("{t}/"))
}

/// Whether `relative` (a forward-slashed zip entry path) is safe to join onto
/// the extraction root. Rejects parent-dir, root, and drive-prefix components,
/// each of which would let `Path::join` escape the destination.
fn is_safe_zip_path(relative: &str) -> bool {
    !Path::new(relative).components().any(|c| {
        matches!(
            c,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    })
}

/// Copies one zip entry to `out`, charging it against `remaining`.
fn copy_entry_capped(
    entry: impl std::io::Read,
    out: &mut impl std::io::Write,
    remaining: &mut u64,
) -> Result<()> {
    // Reading budget + 1 proves the archive exceeds it without ever writing
    // unbounded data; the partial file dies with the staging dir.
    let copied = std::io::copy(&mut entry.take(*remaining + 1), out)?;
    if copied > *remaining {
        return Err(Error::Extract(
            "archive exceeds the decompressed-size budget (possible zip bomb)".to_owned(),
        ));
    }
    *remaining -= copied;
    Ok(())
}

/// Extracts `package` into `dest`, stripping a single shared top-level
/// directory so `chrome.exe` lands directly in `dest`. Entries that would
/// escape `dest` are skipped.
pub fn extract_zip(package: &Path, dest: &Path) -> Result<()> {
    extract_zip_with_budget(package, dest, MAX_DECOMPRESSED_BYTES)
}

fn extract_zip_with_budget(package: &Path, dest: &Path, budget: u64) -> Result<()> {
    let mut remaining = budget;
    let file = File::open(package)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| Error::Extract(e.to_string()))?;

    let names: Vec<String> = (0..archive.len())
        .map(|i| {
            archive
                .by_index(i)
                .map(|e| e.name().replace('\\', "/"))
                .map_err(|e| Error::Extract(e.to_string()))
        })
        .collect::<Result<_>>()?;
    let strip = common_top_dir(&names);

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| Error::Extract(e.to_string()))?;
        let name = entry.name().replace('\\', "/");
        if name.contains("..") || name.starts_with('/') {
            continue;
        }
        let relative = strip
            .as_deref()
            .and_then(|prefix| name.strip_prefix(prefix))
            .unwrap_or(&name)
            .trim_start_matches('/');
        if relative.is_empty() {
            continue;
        }
        if !is_safe_zip_path(relative) {
            eprintln!("warning: skipping zip entry with unsafe path: {name}");
            continue;
        }

        let out = dest.join(relative);
        if entry.is_dir() || name.ends_with('/') {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out_file = File::create(&out)?;
            copy_entry_capped(&mut entry, &mut out_file, &mut remaining)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn common_top_dir_detects_single_shared_root() {
        let names = vec![
            "chrome-win/chrome.exe".to_owned(),
            "chrome-win/locales/en-US.pak".to_owned(),
        ];
        assert_eq!(common_top_dir(&names).as_deref(), Some("chrome-win/"));
    }

    #[test]
    fn common_top_dir_is_none_for_multiple_roots() {
        let names = vec!["a/chrome.exe".to_owned(), "b/chrome.dll".to_owned()];
        assert_eq!(common_top_dir(&names), None);
    }

    #[test]
    fn extract_strips_the_chrome_win_directory() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("chrome-win.zip");
        make_zip(
            &zip_path,
            &[
                ("chrome-win/chrome.exe", b"exe"),
                ("chrome-win/locales/en-US.pak", b"pak"),
            ],
        );
        let dest = dir.path().join("stage");
        extract_zip(&zip_path, &dest).unwrap();
        assert_eq!(std::fs::read(dest.join("chrome.exe")).unwrap(), b"exe");
        assert_eq!(
            std::fs::read(dest.join("locales/en-US.pak")).unwrap(),
            b"pak"
        );
    }

    #[test]
    fn extract_skips_path_traversal_entries() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("evil.zip");
        make_zip(
            &zip_path,
            &[("../escape.txt", b"bad"), ("chrome.exe", b"ok")],
        );
        let dest = dir.path().join("stage");
        extract_zip(&zip_path, &dest).unwrap();
        assert!(!dir.path().join("escape.txt").exists());
        assert!(dest.join("chrome.exe").exists());
    }

    #[test]
    fn unsafe_paths_are_rejected() {
        assert!(!is_safe_zip_path("C:/Windows/evil.dll"));
        assert!(!is_safe_zip_path("/etc/passwd"));
        assert!(!is_safe_zip_path("a/../../b"));
        assert!(is_safe_zip_path("locales/en-US.pak"));
    }

    #[test]
    fn extract_rejects_an_archive_exceeding_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("bomb.zip");
        make_zip(&zip_path, &[("chrome.exe", &[0u8; 1000])]);
        let err = extract_zip_with_budget(&zip_path, &dir.path().join("stage"), 100)
            .expect_err("over-budget archive must fail");
        assert!(matches!(err, Error::Extract(_)));
    }
}
