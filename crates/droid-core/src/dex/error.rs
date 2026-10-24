//! Structured errors for DEX parsing.

use std::fmt;

/// Why a DEX file (or part of one) was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DexError {
    /// The input is larger than [`crate::dex::DexLimits::max_file_size`].
    FileTooLarge { size: usize, max: usize },
    /// A read ran past the end of the data.
    UnexpectedEof { offset: usize },
    /// The file does not start with `dex\n<digits>\0`.
    BadMagic,
    /// The DEX version is well-formed but not supported.
    UnsupportedVersion([u8; 3]),
    /// `header_size` is not `0x70`.
    BadHeaderSize(u32),
    /// `endian_tag` is not the little-endian constant.
    UnsupportedEndianTag(u32),
    /// `file_size` in the header differs from the actual length.
    FileSizeMismatch { declared: u32, actual: usize },
    /// The Adler-32 checksum in the header does not match the contents.
    ChecksumMismatch { declared: u32, computed: u32 },
    /// A table or the data section does not fit inside the file.
    SectionOutOfBounds {
        section: &'static str,
        offset: u32,
        count: u32,
    },
    /// An index refers past the end of the table it indexes.
    IndexOutOfRange {
        kind: &'static str,
        index: u32,
        limit: u32,
    },
    /// A file offset points outside the file.
    OffsetOutOfBounds { what: &'static str, offset: u32 },
    /// A ULEB128 value is too long or does not fit in 32 bits.
    InvalidUleb128 { offset: usize },
    /// A string data item is malformed.
    InvalidStringData { offset: u32 },
    /// A valid but unsupported feature was found.
    Unsupported(&'static str),
    /// A structural rule was violated.
    Malformed(&'static str),
}

impl fmt::Display for DexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileTooLarge { size, max } => {
                write!(f, "dex file is {size} bytes, larger than the {max} byte limit")
            }
            Self::UnexpectedEof { offset } => {
                write!(f, "unexpected end of dex data at offset {offset:#x}")
            }
            Self::BadMagic => write!(f, "not a dex file: bad magic"),
            Self::UnsupportedVersion(v) => write!(
                f,
                "unsupported dex version {}",
                String::from_utf8_lossy(v)
            ),
            Self::BadHeaderSize(size) => write!(f, "unexpected dex header size {size:#x}"),
            Self::UnsupportedEndianTag(tag) => write!(f, "unsupported endian tag {tag:#010x}"),
            Self::FileSizeMismatch { declared, actual } => write!(
                f,
                "header declares {declared} bytes but the file has {actual}"
            ),
            Self::ChecksumMismatch { declared, computed } => write!(
                f,
                "checksum mismatch: header has {declared:#010x}, computed {computed:#010x}"
            ),
            Self::SectionOutOfBounds {
                section,
                offset,
                count,
            } => write!(
                f,
                "{section} section (offset {offset:#x}, {count} items) does not fit in the file"
            ),
            Self::IndexOutOfRange { kind, index, limit } => {
                write!(f, "{kind} index {index} out of range (limit {limit})")
            }
            Self::OffsetOutOfBounds { what, offset } => {
                write!(f, "{what} offset {offset:#x} is outside the file")
            }
            Self::InvalidUleb128 { offset } => {
                write!(f, "invalid ULEB128 value at offset {offset:#x}")
            }
            Self::InvalidStringData { offset } => {
                write!(f, "malformed string data at offset {offset:#x}")
            }
            Self::Unsupported(what) => write!(f, "unsupported dex feature: {what}"),
            Self::Malformed(what) => write!(f, "malformed dex: {what}"),
        }
    }
}

impl std::error::Error for DexError {}
