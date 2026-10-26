//! Errors for APK inspection.

use super::manifest::ManifestError;
use std::fmt;
use std::io;
use std::path::PathBuf;

/// Why an archive entry name was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsafeEntryReason {
    Empty,
    TooLong,
    NulByte,
    Backslash,
    Absolute,
    DriveLetter,
    ParentTraversal,
}

impl fmt::Display for UnsafeEntryReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Empty => "name is empty",
            Self::TooLong => "name is too long",
            Self::NulByte => "name contains a NUL byte",
            Self::Backslash => "name contains a backslash",
            Self::Absolute => "name is an absolute path",
            Self::DriveLetter => "name starts with a drive letter",
            Self::ParentTraversal => "name contains a parent directory component",
        };
        f.write_str(text)
    }
}

/// Why an APK could not be inspected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApkError {
    /// A file could not be opened or read.
    Io {
        path: Option<PathBuf>,
        kind: io::ErrorKind,
    },
    /// The APK is larger than [`crate::apk::ApkLimits::max_apk_size`].
    FileTooLarge { size: u64, max: u64 },
    /// The data is not a readable ZIP archive.
    InvalidArchive { reason: String },
    /// The archive has more entries than allowed.
    TooManyEntries { count: usize, max: usize },
    /// An entry name could be used for path traversal or is otherwise unsafe.
    UnsafeEntryName {
        name: String,
        reason: UnsafeEntryReason,
    },
    /// Two entries share the same name.
    DuplicateEntry { name: String },
    /// A required entry is absent.
    MissingEntry { name: &'static str },
    /// An entry is larger than the limit for its kind.
    EntryTooLarge { name: String, size: u64, max: u64 },
    /// An entry exists but could not be read (corrupt, encrypted, unsupported).
    EntryUnreadable { name: String, reason: String },
    /// The total amount of data read from the archive exceeded the budget.
    TotalSizeExceeded { max: u64 },
    /// `AndroidManifest.xml` could not be parsed.
    Manifest(ManifestError),
}

impl fmt::Display for ApkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                path: Some(path),
                kind,
            } => write!(f, "cannot read {}: {kind}", path.display()),
            Self::Io { path: None, kind } => write!(f, "read error: {kind}"),
            Self::FileTooLarge { size, max } => {
                write!(f, "apk is {size} bytes, larger than the {max} byte limit")
            }
            Self::InvalidArchive { reason } => write!(f, "not a valid zip archive: {reason}"),
            Self::TooManyEntries { count, max } => {
                write!(
                    f,
                    "archive has {count} entries, more than the limit of {max}"
                )
            }
            Self::UnsafeEntryName { name, reason } => {
                write!(f, "unsafe archive entry name {name:?}: {reason}")
            }
            Self::DuplicateEntry { name } => write!(f, "duplicate archive entry {name:?}"),
            Self::MissingEntry { name } => write!(f, "required entry {name} is missing"),
            Self::EntryTooLarge { name, size, max } => write!(
                f,
                "entry {name:?} is {size} bytes, larger than the {max} byte limit"
            ),
            Self::EntryUnreadable { name, reason } => {
                write!(f, "cannot read entry {name:?}: {reason}")
            }
            Self::TotalSizeExceeded { max } => {
                write!(f, "archive read budget of {max} bytes exceeded")
            }
            Self::Manifest(e) => write!(f, "invalid manifest: {e}"),
        }
    }
}

impl std::error::Error for ApkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Manifest(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ManifestError> for ApkError {
    fn from(error: ManifestError) -> Self {
        Self::Manifest(error)
    }
}
