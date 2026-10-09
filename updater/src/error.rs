//! Error type shared by every update step.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// The network is unreachable or GitHub rate-limited us. The existing
    /// install is untouched and still launchable.
    #[error("offline: {0}")]
    Offline(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("could not parse release metadata: {0}")]
    Parse(String),
    /// Integrity or provenance could not be established. Fail-closed.
    #[error("verification failed: {0}")]
    Verification(String),
    /// The release does not look like a Cromite Windows build we understand.
    #[error("incompatible release: {0}")]
    Compatibility(String),
    #[error("extraction failed: {0}")]
    Extract(String),
    #[error("Cromite is running from this folder; close it and run the updater again")]
    InUse,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
