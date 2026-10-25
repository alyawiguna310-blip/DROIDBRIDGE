//! Bounded, read-only access to the entries of an APK (ZIP) archive.

use super::error::{ApkError, UnsafeEntryReason};
use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use zip::ZipArchive;

/// Longest accepted entry name, in bytes.
pub(crate) const MAX_ENTRY_NAME_LEN: usize = 1024;

const EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const EOCD_MIN_LEN: usize = 22;
const MAX_COMMENT_LEN: usize = 65_535;

/// Metadata of one archive entry, taken from the central directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntryInfo {
    pub(crate) name: String,
    /// Uncompressed size as declared by the archive (not trusted when reading).
    pub(crate) size: u64,
    pub(crate) is_dir: bool,
}

/// Caps the total number of bytes read from one archive.
#[derive(Debug)]
pub(crate) struct ReadBudget {
    max: u64,
    remaining: u64,
}

impl ReadBudget {
    pub(crate) fn new(max: u64) -> Self {
        Self {
            max,
            remaining: max,
        }
    }

    fn charge(&mut self, bytes: u64) -> Result<(), ApkError> {
        if bytes > self.remaining {
            return Err(ApkError::TotalSizeExceeded { max: self.max });
        }
        self.remaining -= bytes;
        Ok(())
    }
}

pub(crate) struct ApkArchive<R: Read + Seek> {
    zip: ZipArchive<R>,
    entries: Vec<EntryInfo>,
    index: HashMap<String, usize>,
}

impl<R: Read + Seek> ApkArchive<R> {
    /// Open the archive and validate every entry name.
    ///
    /// The entry count is checked against `max_entries` from the end-of-central-
    /// directory record before the ZIP library parses the directory, and again
    /// afterwards (ZIP64 archives report their count only after parsing).
    pub(crate) fn open(mut reader: R, max_entries: usize) -> Result<Self, ApkError> {
        let declared = eocd_entry_count(&mut reader).map_err(|e| ApkError::Io {
            path: None,
            kind: e.kind(),
        })?;
        if let Some(count) = declared {
            if usize::from(count) > max_entries {
                return Err(ApkError::TooManyEntries {
                    count: usize::from(count),
                    max: max_entries,
                });
            }
        }

        let mut zip = ZipArchive::new(reader).map_err(|e| ApkError::InvalidArchive {
            reason: e.to_string(),
        })?;
        let count = zip.len();
        if count > max_entries {
            return Err(ApkError::TooManyEntries {
                count,
                max: max_entries,
            });
        }

        let mut entries = Vec::with_capacity(count);
        let mut index = HashMap::with_capacity(count);
        for i in 0..count {
            let file = zip.by_index_raw(i).map_err(|e| ApkError::InvalidArchive {
                reason: e.to_string(),
            })?;
            let name = file.name().to_owned();
            validate_entry_name(&name)?;
            let info = EntryInfo {
                name: name.clone(),
                size: file.size(),
                is_dir: file.is_dir(),
            };
            if index.insert(name.clone(), i).is_some() {
                return Err(ApkError::DuplicateEntry { name });
            }
            entries.push(info);
        }

        Ok(Self {
            zip,
            entries,
            index,
        })
    }

    pub(crate) fn entries(&self) -> &[EntryInfo] {
        &self.entries
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }

    /// Read one entry into memory, never more than `max_size` bytes.
    ///
    /// The declared size is checked first, and the read itself is capped at
    /// `max_size + 1` bytes so an entry that lies about its size (a ZIP bomb)
    /// still cannot exceed the limit.
    pub(crate) fn read_entry(
        &mut self,
        name: &str,
        max_size: u64,
        budget: &mut ReadBudget,
    ) -> Result<Vec<u8>, ApkError> {
        let unreadable = |reason: String| ApkError::EntryUnreadable {
            name: name.to_owned(),
            reason,
        };
        let idx = *self
            .index
            .get(name)
            .ok_or_else(|| unreadable("entry not found".to_owned()))?;
        let mut file = self
            .zip
            .by_index(idx)
            .map_err(|e| unreadable(e.to_string()))?;

        let declared = file.size();
        if declared > max_size {
            return Err(ApkError::EntryTooLarge {
                name: name.to_owned(),
                size: declared,
                max: max_size,
            });
        }

        let mut buf = Vec::new();
        (&mut file)
            .take(max_size.saturating_add(1))
            .read_to_end(&mut buf)
            .map_err(|e| unreadable(e.to_string()))?;
        let actual = buf.len() as u64;
        if actual > max_size {
            return Err(ApkError::EntryTooLarge {
                name: name.to_owned(),
                size: actual,
                max: max_size,
            });
        }
        budget.charge(actual)?;
        Ok(buf)
    }
}

/// Reject entry names that could escape a directory if the archive were ever
/// extracted, or that are otherwise malformed. Nothing is extracted by this
/// crate; the check keeps unsafe names from reaching any later stage.
pub(crate) fn validate_entry_name(name: &str) -> Result<(), ApkError> {
    let reason = if name.is_empty() {
        Some(UnsafeEntryReason::Empty)
    } else if name.len() > MAX_ENTRY_NAME_LEN {
        Some(UnsafeEntryReason::TooLong)
    } else if name.contains(char::from(0)) {
        Some(UnsafeEntryReason::NulByte)
    } else if name.contains(char::from(0x5c)) {
        Some(UnsafeEntryReason::Backslash)
    } else if name.starts_with('/') {
        Some(UnsafeEntryReason::Absolute)
    } else if has_drive_letter(name) {
        Some(UnsafeEntryReason::DriveLetter)
    } else if name.split('/').any(|component| component == "..") {
        Some(UnsafeEntryReason::ParentTraversal)
    } else {
        None
    };
    match reason {
        Some(reason) => Err(ApkError::UnsafeEntryName {
            name: name.to_owned(),
            reason,
        }),
        None => Ok(()),
    }
}

fn has_drive_letter(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Read the entry count from the end-of-central-directory record, if one can
/// be found in the last 64 KiB. Leaves the reader at the start.
fn eocd_entry_count<R: Read + Seek>(reader: &mut R) -> io::Result<Option<u16>> {
    let len = reader.seek(SeekFrom::End(0))?;
    let mut found = None;
    if len >= EOCD_MIN_LEN as u64 {
        let tail_len = len.min((EOCD_MIN_LEN + MAX_COMMENT_LEN) as u64);
        reader.seek(SeekFrom::Start(len - tail_len))?;
        let mut tail = vec![0u8; tail_len as usize];
        reader.read_exact(&mut tail)?;
        let last = tail.len() - EOCD_MIN_LEN;
        found = (0..=last)
            .rev()
            .find(|&i| tail[i..i + 4] == EOCD_SIGNATURE)
            .map(|i| u16::from_le_bytes([tail[i + 10], tail[i + 11]]));
    }
    reader.seek(SeekFrom::Start(0))?;
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reason(name: &str) -> Option<UnsafeEntryReason> {
        match validate_entry_name(name) {
            Ok(()) => None,
            Err(ApkError::UnsafeEntryName { reason, .. }) => Some(reason),
            Err(other) => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn accepts_ordinary_and_unusual_but_safe_names() {
        for name in [
            "AndroidManifest.xml",
            "classes.dex",
            "res/layout/main.xml",
            "res/",
            "assets/with space.txt",
            "assets/日本語.txt",
            "assets/..hidden",
            "assets/a..b",
            "META-INF/CERT.RSA",
            "a:b",
        ] {
            assert_eq!(reason(name), None, "name {name:?}");
        }
    }

    #[test]
    fn rejects_unsafe_names_with_a_specific_reason() {
        let backslash = String::from_utf8(vec![b'a', 0x5c, b'b']).unwrap();
        let nul = String::from_utf8(vec![b'a', 0, b'b']).unwrap();
        assert_eq!(reason(""), Some(UnsafeEntryReason::Empty));
        assert_eq!(reason(&nul), Some(UnsafeEntryReason::NulByte));
        assert_eq!(reason(&backslash), Some(UnsafeEntryReason::Backslash));
        assert_eq!(reason("/etc/passwd"), Some(UnsafeEntryReason::Absolute));
        assert_eq!(reason("C:/x"), Some(UnsafeEntryReason::DriveLetter));
        assert_eq!(reason("../x"), Some(UnsafeEntryReason::ParentTraversal));
        assert_eq!(reason("a/../x"), Some(UnsafeEntryReason::ParentTraversal));
        assert_eq!(reason("a/.."), Some(UnsafeEntryReason::ParentTraversal));
        assert_eq!(
            reason(&"a".repeat(MAX_ENTRY_NAME_LEN + 1)),
            Some(UnsafeEntryReason::TooLong)
        );
        assert_eq!(reason(&"a".repeat(MAX_ENTRY_NAME_LEN)), None);
    }

    #[test]
    fn read_budget_is_enforced() {
        let mut budget = ReadBudget::new(10);
        assert_eq!(budget.charge(6), Ok(()));
        assert_eq!(budget.charge(4), Ok(()));
        assert_eq!(
            budget.charge(1),
            Err(ApkError::TotalSizeExceeded { max: 10 })
        );
    }
}
