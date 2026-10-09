//! Resolving the latest Cromite Windows build from GitHub releases.
//!
//! Adapted from Nomad Launcher `core/src/browsers/github.rs` and
//! `core/src/browsers/ungoogled.rs` (MIT, see `THIRD_PARTY_NOTICES.md`).
//!
//! Cromite publishes no GPG signature, so — like Nomad's Ungoogled Chromium
//! vertical — integrity rests on the SHA-256 `digest` GitHub records for each
//! release asset, plus an upload-timeline check that catches an asset being
//! swapped after the release was published. Both are mandatory.

use serde::Deserialize;

use crate::error::{Error, Result};

pub const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/uazo/cromite/releases/latest";

/// The Windows x64 build asset name used by every Cromite release.
const WINDOWS_ASSET: &str = "chrome-win.zip";

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub prerelease: bool,
    pub published_at: Option<String>,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
    /// GitHub-recorded content digest, e.g. `"sha256:abcd…"`.
    pub digest: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Everything needed to download and verify one release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfo {
    /// Full release tag without the leading `v`, e.g.
    /// `153.0.8010.37-11507ac1061b5ea227806f5e84db5a57df6ccf6a`.
    pub tag: String,
    /// Chromium version, e.g. `153.0.8010.37`.
    pub chromium_version: String,
    pub download_url: String,
    pub sha256: String,
    pub size: u64,
    /// Release date (`YYYY-MM-DD`), shown in the window.
    pub published: Option<String>,
}

/// Maps a `reqwest` error: connection failures, timeouts, and HTTP 403 (the
/// GitHub API rate limit) are [`Error::Offline`], so callers can keep using
/// the existing install.
pub fn map_network_err(e: reqwest::Error) -> Error {
    if e.is_connect() || e.is_timeout() {
        Error::Offline(e.to_string())
    } else if e.status() == Some(reqwest::StatusCode::FORBIDDEN) {
        Error::Offline(format!("GitHub API rate limit exceeded: {e}"))
    } else {
        Error::Network(e.to_string())
    }
}

/// Fetches and parses the latest release from `url`.
pub fn fetch_latest(client: &reqwest::blocking::Client, url: &str) -> Result<VersionInfo> {
    let body = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(map_network_err)?
        .error_for_status()
        .map_err(map_network_err)?
        .text()
        .map_err(map_network_err)?;
    let release: Release = serde_json::from_str(&body).map_err(|e| Error::Parse(e.to_string()))?;
    parse_release(&release)
}

/// Builds a [`VersionInfo`] from a release, enforcing every gate that can be
/// checked before downloading: asset present, digest present and well-formed,
/// clean upload timeline, and a parseable Chromium version.
pub fn parse_release(release: &Release) -> Result<VersionInfo> {
    if release.prerelease {
        return Err(Error::Compatibility(format!(
            "release {} is marked as a prerelease",
            release.tag_name
        )));
    }

    let asset = release
        .assets
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(WINDOWS_ASSET))
        .ok_or_else(|| {
            Error::Parse(format!(
                "no {WINDOWS_ASSET} asset in release {}",
                release.tag_name
            ))
        })?;

    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            Error::Verification(format!(
                "release {} has no usable sha256 digest for {WINDOWS_ASSET}; refusing an unverifiable package",
                release.tag_name
            ))
        })?;

    if asset_provenance_suspect(asset, release) {
        return Err(Error::Verification(format!(
            "{WINDOWS_ASSET} in release {} was uploaded or replaced after publication; skipping this update",
            release.tag_name
        )));
    }

    let tag = release.tag_name.trim_start_matches('v').to_owned();
    let chromium_version = tag.split('-').next().unwrap_or(&tag).to_owned();
    validate_chromium_version(&chromium_version)?;

    Ok(VersionInfo {
        tag,
        chromium_version,
        download_url: asset.browser_download_url.clone(),
        sha256,
        size: asset.size,
        published: release
            .published_at
            .as_deref()
            .and_then(|t| t.get(..10))
            .map(str::to_owned),
    })
}

/// Compatibility gate on the version string: four dot-separated integers with
/// a non-zero major (Nomad's `validate_update` for Chromium).
fn validate_chromium_version(version: &str) -> Result<()> {
    let parts: Vec<u32> = version
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| {
            Error::Compatibility(format!("could not parse Chromium version `{version}`"))
        })?;
    if parts.len() != 4 || parts[0] == 0 {
        return Err(Error::Compatibility(format!(
            "unexpected Chromium version `{version}`"
        )));
    }
    Ok(())
}

// ── Asset provenance (verbatim logic from Nomad) ──────────────────────────────

/// Tolerance for `updated_at` exceeding `created_at`: GitHub finalises upload
/// metadata slightly after creation (Cromite's ~200 MB zip takes ~90 s).
const ASSET_REUPLOAD_TOLERANCE_SECS: i64 = 5 * 60;

/// Tolerance for `created_at` falling after `published_at`.
const ASSET_LATE_UPLOAD_TOLERANCE_SECS: i64 = 48 * 60 * 60;

/// Returns `true` when the asset's upload timeline does not match the
/// release's publication — evidence of a release-asset swap, which GitHub
/// permits without touching the tag. Missing timestamps count as suspect.
pub fn asset_provenance_suspect(asset: &ReleaseAsset, release: &Release) -> bool {
    let (Some(created), Some(updated), Some(published)) = (
        asset.created_at.as_deref().and_then(timestamp_epoch),
        asset.updated_at.as_deref().and_then(timestamp_epoch),
        release.published_at.as_deref().and_then(timestamp_epoch),
    ) else {
        return true;
    };
    updated - created > ASSET_REUPLOAD_TOLERANCE_SECS
        || created - published > ASSET_LATE_UPLOAD_TOLERANCE_SECS
}

/// Parses a GitHub API timestamp (`YYYY-MM-DDTHH:MM:SSZ`) into Unix seconds.
fn timestamp_epoch(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let s = ts.get(range)?;
        if !s.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, min, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + min * 60 + sec)
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "sha256:c95d1c679142a421df7de2cab090539ccb4158f69410dc1878735fb8f60e0fab";

    /// Shaped after the real v153.0.8010.37 release metadata.
    fn release_json(digest: &str, created: &str, updated: &str) -> String {
        format!(
            r#"{{"tag_name":"v153.0.8010.37-11507ac1061b5ea227806f5e84db5a57df6ccf6a",
                "prerelease":false,"published_at":"2026-09-15T11:51:14Z",
                "assets":[
                  {{"name":"chrome-lin64.tar.gz","browser_download_url":"https://x/lin","size":1,
                    "digest":"sha256:{z}","created_at":"2026-09-15T11:36:59Z","updated_at":"2026-09-15T11:39:07Z"}},
                  {{"name":"chrome-win.zip","browser_download_url":"https://x/chrome-win.zip","size":204392488,
                    "digest":{digest},"created_at":"{created}","updated_at":"{updated}"}}
                ]}}"#,
            z = "0".repeat(64)
        )
    }

    fn parse(json: &str) -> Result<VersionInfo> {
        parse_release(&serde_json::from_str(json).unwrap())
    }

    #[test]
    fn parses_the_real_release_shape() {
        let info = parse(&release_json(
            &format!("\"{DIGEST}\""),
            "2026-09-15T11:34:50Z",
            "2026-09-15T11:36:22Z",
        ))
        .unwrap();
        assert_eq!(
            info.tag,
            "153.0.8010.37-11507ac1061b5ea227806f5e84db5a57df6ccf6a"
        );
        assert_eq!(info.chromium_version, "153.0.8010.37");
        assert_eq!(info.download_url, "https://x/chrome-win.zip");
        assert_eq!(info.sha256, DIGEST.trim_start_matches("sha256:"));
        assert_eq!(info.size, 204_392_488);
        assert_eq!(info.published.as_deref(), Some("2026-09-15"));
    }

    #[test]
    fn rejects_a_missing_digest() {
        let err = parse(&release_json(
            "null",
            "2026-09-15T11:34:50Z",
            "2026-09-15T11:36:22Z",
        ))
        .expect_err("no digest must fail closed");
        assert!(matches!(err, Error::Verification(_)));
    }

    #[test]
    fn rejects_a_malformed_digest() {
        let err = parse(&release_json(
            "\"sha256:nothex\"",
            "2026-09-15T11:34:50Z",
            "2026-09-15T11:36:22Z",
        ))
        .expect_err("malformed digest must fail closed");
        assert!(matches!(err, Error::Verification(_)));
    }

    #[test]
    fn rejects_an_asset_replaced_after_publication() {
        let err = parse(&release_json(
            &format!("\"{DIGEST}\""),
            "2026-09-15T11:34:50Z",
            "2026-09-20T09:00:00Z",
        ))
        .expect_err("in-place swap must be refused");
        assert!(matches!(err, Error::Verification(_)));
    }

    #[test]
    fn rejects_a_release_without_a_windows_asset() {
        let json =
            r#"{"tag_name":"v1.0.0.0-abc","published_at":"2026-09-15T11:51:14Z","assets":[]}"#;
        assert!(matches!(parse(json), Err(Error::Parse(_))));
    }

    #[test]
    fn chromium_version_gate() {
        assert!(validate_chromium_version("153.0.8010.37").is_ok());
        assert!(validate_chromium_version("0.0.0.1").is_err());
        assert!(validate_chromium_version("153.0").is_err());
        assert!(validate_chromium_version("latest").is_err());
    }

    #[test]
    fn timestamp_epoch_known_values() {
        assert_eq!(timestamp_epoch("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(timestamp_epoch("2000-02-29T23:59:59Z"), Some(951_868_799));
        assert_eq!(timestamp_epoch("2026-09-15T11:51:14Z"), Some(1_789_473_074));
    }

    #[test]
    fn timestamp_epoch_rejects_non_github_shapes() {
        for ts in [
            "",
            "2026-05-11",
            "2026-05-11T16:40:22",
            "2026-05-11T16:40:22.123Z",
            "2026-05-11T16:40:22+02:00",
            "2026-13-11T16:40:22Z",
            "2026-05-11T24:40:22Z",
        ] {
            assert_eq!(timestamp_epoch(ts), None, "must reject {ts:?}");
        }
    }
}
