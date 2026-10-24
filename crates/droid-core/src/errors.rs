//! Structured errors for the DROIDBRIDGE core.
//!
//! Callers should match on variants rather than parse `Display` output.

use std::fmt;
use std::io;
use std::path::PathBuf;

/// Why a profile identifier was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileIdError {
    /// The identifier is empty or whitespace only.
    Empty,
    /// The first character is not an ASCII letter or digit.
    InvalidStart,
    /// The identifier contains a character outside `A-Z a-z 0-9 _ -`.
    InvalidChar(char),
    /// The identifier is longer than the allowed maximum.
    TooLong { max: usize },
}

impl fmt::Display for ProfileIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "identifier cannot be empty"),
            Self::InvalidStart => {
                write!(f, "identifier must start with an ASCII letter or digit")
            }
            Self::InvalidChar(c) => write!(f, "identifier contains unsupported character {c:?}"),
            Self::TooLong { max } => write!(f, "identifier is longer than {max} characters"),
        }
    }
}

impl std::error::Error for ProfileIdError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    InvalidJson(String),
    EmptyField(&'static str),
    EmptyAbiList,
    EmptyAbiEntry,
    ZeroDisplayValue(&'static str),
    /// The JSON has no `id` field. Older profiles must be migrated explicitly
    /// with `DeviceProfile::from_legacy_json`.
    MissingId,
    /// The `id` field is present but not a valid stable identifier.
    InvalidId {
        id: String,
        problem: ProfileIdError,
    },
    /// A legacy profile already carries an `id` that differs from the one the
    /// caller asked to assign.
    IdMismatch {
        supplied: String,
        found: String,
    },
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(message) => write!(f, "invalid device profile: {message}"),
            Self::EmptyField(field) => write!(f, "device profile field cannot be empty: {field}"),
            Self::EmptyAbiList => write!(f, "device profile must report at least one ABI"),
            Self::EmptyAbiEntry => write!(f, "device profile ABI entries cannot be empty"),
            Self::ZeroDisplayValue(field) => {
                write!(f, "device profile field must be nonzero: {field}")
            }
            Self::MissingId => write!(
                f,
                "device profile is missing the required \"id\" field; \
                 legacy profiles must be migrated explicitly with DeviceProfile::from_legacy_json"
            ),
            Self::InvalidId { id, problem } => {
                write!(f, "invalid device profile id {id:?}: {problem}")
            }
            Self::IdMismatch { supplied, found } => write!(
                f,
                "legacy profile already contains id {found:?}, which does not match the supplied id {supplied:?}"
            ),
        }
    }
}

impl std::error::Error for ProfileError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    InvalidJson(String),
    UnsupportedSchema(u32),
    EmptyProfileName,
    /// `active_profile` is not a valid profile identifier.
    InvalidActiveProfile {
        id: String,
        problem: ProfileIdError,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(message) => write!(f, "invalid configuration: {message}"),
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported configuration schema version: {version}")
            }
            Self::EmptyProfileName => write!(f, "active profile name cannot be empty"),
            Self::InvalidActiveProfile { id, problem } => {
                write!(f, "invalid active profile id {id:?}: {problem}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Errors from [`crate::ProfileRegistry`] operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// The profile failed validation or could not be parsed.
    InvalidProfile(ProfileError),
    /// The configuration failed validation.
    InvalidConfig(ConfigError),
    /// A profile with this id is already registered.
    DuplicateId(String),
    /// No profile is registered under this id.
    NotFound(String),
    /// A profile file could not be read.
    Io { path: PathBuf, kind: io::ErrorKind },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProfile(e) => write!(f, "cannot register profile: {e}"),
            Self::InvalidConfig(e) => write!(f, "cannot apply configuration: {e}"),
            Self::DuplicateId(id) => write!(f, "a profile with id {id:?} is already registered"),
            Self::NotFound(id) => write!(f, "no profile registered with id {id:?}"),
            Self::Io { path, kind } => {
                write!(f, "cannot read profile file {}: {kind}", path.display())
            }
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidProfile(e) => Some(e),
            Self::InvalidConfig(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ProfileError> for RegistryError {
    fn from(error: ProfileError) -> Self {
        Self::InvalidProfile(error)
    }
}

impl From<ConfigError> for RegistryError {
    fn from(error: ConfigError) -> Self {
        Self::InvalidConfig(error)
    }
}
