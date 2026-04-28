//! Update-check status tracking for the TUI footer.

/// Status of the background update check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// No check result yet.
    Unknown,
    /// The running version matches the latest known release.
    UpToDate,
    /// A newer release is available.
    Available { latest: String },
}
