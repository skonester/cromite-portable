//! Streaming HTTPS download and SHA-256 verification.
//!
//! Adapted from Nomad Launcher `core/src/downloader.rs` (MIT, see
//! `THIRD_PARTY_NOTICES.md`), using reqwest's blocking client.
//!
//! A download is written to `<dest>.tmp` and renamed onto `dest` only after a
//! complete transfer, so `dest` is never partially written.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::github::map_network_err;

/// Hard ceiling on a single download: headroom over Cromite's ~200 MB zip
/// while bounding an endpoint that streams without end.
const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;

const MAX_REDIRECTS: usize = 10;

/// `true` when following `next` would step down from https to plain http.
fn is_scheme_downgrade(next: &reqwest::Url, previous: &[reqwest::Url]) -> bool {
    next.scheme() != "https" && previous.iter().any(|u| u.scheme() == "https")
}

/// Builds the HTTP client used for both the API call and the download.
pub fn client() -> Result<reqwest::blocking::Client> {
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else if is_scheme_downgrade(attempt.url(), attempt.previous()) {
            attempt.error("refusing redirect downgrade from https to http")
        } else {
            attempt.follow()
        }
    });
    reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "cromite-portable-updater/",
            env!("CARGO_PKG_VERSION")
        ))
        .redirect(policy)
        .connect_timeout(Duration::from_secs(30))
        // The blocking client's 30 s default covers the whole body; a 200 MB
        // download on a slow link legitimately takes longer.
        .timeout(None)
        .build()
        .map_err(|e| Error::Network(e.to_string()))
}

/// Streams `url` to `dest`, calling `on_progress(downloaded, total)`.
pub fn download(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path(dest);
    match download_to_tmp(client, url, &tmp, &mut on_progress, MAX_DOWNLOAD_BYTES) {
        Ok(()) => crate::install::retry_transient(|| std::fs::rename(&tmp, dest))
            .map_err(crate::install::io_context("saving the download")),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn download_to_tmp(
    client: &reqwest::blocking::Client,
    url: &str,
    tmp: &Path,
    on_progress: &mut impl FnMut(u64, Option<u64>),
    max_bytes: u64,
) -> Result<()> {
    let mut response = client
        .get(url)
        .send()
        .map_err(map_network_err)?
        .error_for_status()
        .map_err(map_network_err)?;

    let total = response.content_length();
    if total.is_some_and(|len| len > max_bytes) {
        return Err(Error::Network(format!(
            "{url}: declared size exceeds the {max_bytes} B limit"
        )));
    }

    let mut file = std::io::BufWriter::new(std::fs::File::create(tmp)?);
    let mut buf = vec![0u8; 256 * 1024];
    let mut downloaded: u64 = 0;
    loop {
        let n = response
            .read(&mut buf)
            .map_err(|e| Error::Network(format!("{url}: {e}")))?;
        if n == 0 {
            break;
        }
        downloaded += n as u64;
        if downloaded > max_bytes {
            return Err(Error::Network(format!(
                "{url}: download exceeded the {max_bytes} B limit"
            )));
        }
        file.write_all(&buf[..n])?;
        on_progress(downloaded, total);
    }
    file.flush()?;
    file.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    Ok(())
}

fn tmp_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    dest.with_file_name(name)
}

/// Hashes the file on disk — the bytes that will actually be extracted.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Fails closed unless the package's SHA-256 matches `expected` (and its size
/// matches `expected_size`, when GitHub reported one).
pub fn verify_package(package: &Path, expected: &str, expected_size: u64) -> Result<()> {
    let actual_size = std::fs::metadata(package)?.len();
    if expected_size != 0 && actual_size != expected_size {
        return Err(Error::Verification(format!(
            "size mismatch: expected {expected_size} B, got {actual_size} B"
        )));
    }
    let actual = sha256_file(package)?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(Error::Verification(format!(
            "SHA-256 mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tmp_path_appends_tmp_suffix() {
        assert_eq!(
            tmp_path(Path::new("dir/pkg.zip")),
            PathBuf::from("dir/pkg.zip.tmp")
        );
    }

    #[test]
    fn scheme_downgrade_is_detected_only_after_an_https_hop() {
        let secure: reqwest::Url = "https://github.com/pkg".parse().unwrap();
        let plain: reqwest::Url = "http://mirror.example/pkg".parse().unwrap();
        assert!(is_scheme_downgrade(&plain, std::slice::from_ref(&secure)));
        assert!(!is_scheme_downgrade(&secure, std::slice::from_ref(&secure)));
        assert!(!is_scheme_downgrade(&plain, std::slice::from_ref(&plain)));
    }

    #[test]
    fn verify_package_accepts_a_matching_hash_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("chrome-win.zip");
        std::fs::write(&pkg, b"abc").unwrap();
        // SHA-256("abc")
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        verify_package(&pkg, expected, 3).unwrap();
        verify_package(&pkg, &expected.to_ascii_uppercase(), 0).unwrap();
    }

    #[test]
    fn verify_package_rejects_a_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("chrome-win.zip");
        std::fs::write(&pkg, b"tampered").unwrap();
        let err = verify_package(&pkg, &"0".repeat(64), 0).unwrap_err();
        assert!(matches!(err, Error::Verification(_)));
    }

    #[test]
    fn verify_package_rejects_a_size_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("chrome-win.zip");
        std::fs::write(&pkg, b"abc").unwrap();
        let err = verify_package(&pkg, &"0".repeat(64), 4).unwrap_err();
        assert!(matches!(err, Error::Verification(_)));
    }
}
