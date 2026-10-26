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

/// Returns whether a classic EOCD entry count is authoritative and exceeds the
/// configured limit. ZIP64 uses 0xFFFF as a sentinel, so that value must be
/// resolved from the ZIP64 end record instead of treated as an actual count.
fn classic_count_exceeds_limit(count: u16, max_entries: usize) -> bool {
    count != u16::MAX && usize::from(count) > max_entries
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
        if let Some(count) = declared
            && classic_count_exceeds_limit(count, max_entries)
        {
            return Err(ApkError::TooManyEntries {
                count: usize::from(count),
                max: max_entries,
            });
        }

        reject_duplicate_raw_names(&mut reader, max_entries)?;

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

/// Reject duplicate raw names before zip::ZipArchive builds its name-keyed map.
fn reject_duplicate_raw_names<R: Read + Seek>(
    reader: &mut R,
    max_entries: usize,
) -> Result<(), ApkError> {
    use std::collections::HashSet;

    const EOCD: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const ZIP64_LOCATOR: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
    const ZIP64_EOCD: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
    const CENTRAL: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
    const DIGITAL_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x05];

    fn invalid(reason: &str) -> ApkError {
        ApkError::InvalidArchive {
            reason: reason.to_owned(),
        }
    }

    fn read_at<R: Read + Seek>(
        reader: &mut R,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<(), ApkError> {
        reader
            .seek(SeekFrom::Start(offset))
            .map_err(|e| ApkError::Io {
                path: None,
                kind: e.kind(),
            })?;
        reader.read_exact(buf).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                invalid("truncated ZIP directory")
            } else {
                ApkError::Io {
                    path: None,
                    kind: e.kind(),
                }
            }
        })
    }

    let result = (|| {
        let len = reader.seek(SeekFrom::End(0)).map_err(|e| ApkError::Io {
            path: None,
            kind: e.kind(),
        })?;

        if len < 22 {
            return Err(invalid("missing ZIP end-of-central-directory record"));
        }

        let tail_len = len.min(22 + 65_535);
        let tail_start = len - tail_len;
        let mut tail = vec![0; tail_len as usize];
        read_at(reader, tail_start, &mut tail)?;

        // Require the declared comment to fit in the file, so a signature
        // occurring inside a comment is not mistaken for the EOCD.
        let eocd_index = (0..=tail.len() - 22)
            .rev()
            .find(|&i| {
                tail[i..i + 4] == EOCD
                    && i + 22 + u16::from_le_bytes([tail[i + 20], tail[i + 21]]) as usize
                        == tail.len()
            })
            .ok_or_else(|| invalid("invalid ZIP end-of-central-directory record"))?;

        let eocd_pos = tail_start + eocd_index as u64;
        let eocd = &tail[eocd_index..];
        let mut count = u16::from_le_bytes([eocd[10], eocd[11]]) as u64;
        let mut cd_size = u32::from_le_bytes(eocd[12..16].try_into().unwrap()) as u64;
        let mut cd_offset = u32::from_le_bytes(eocd[16..20].try_into().unwrap()) as u64;
        let mut cd_end = eocd_pos;

        if count == u16::MAX as u64 || cd_size == u32::MAX as u64 || cd_offset == u32::MAX as u64 {
            if eocd_pos < 20 {
                return Err(invalid("missing ZIP64 locator"));
            }

            let mut locator = [0; 20];
            read_at(reader, eocd_pos - 20, &mut locator)?;
            if locator[..4] != ZIP64_LOCATOR {
                return Err(invalid("missing ZIP64 locator"));
            }

            let zip64_pos = u64::from_le_bytes(locator[8..16].try_into().unwrap());
            let mut z = [0; 56];
            read_at(reader, zip64_pos, &mut z)?;
            if z[..4] != ZIP64_EOCD {
                return Err(invalid("invalid ZIP64 end record"));
            }

            count = u64::from_le_bytes(z[32..40].try_into().unwrap());
            cd_size = u64::from_le_bytes(z[40..48].try_into().unwrap());
            cd_offset = u64::from_le_bytes(z[48..56].try_into().unwrap());
            cd_end = zip64_pos;
        }

        let max = max_entries as u64;
        if count > max {
            return Err(ApkError::TooManyEntries {
                count: usize::try_from(count).unwrap_or(usize::MAX),
                max: max_entries,
            });
        }

        let cd_start = cd_end
            .checked_sub(cd_size)
            .ok_or_else(|| invalid("central directory offset underflow"))?;

        // Account for a prepended stub (self-extracting ZIP).
        let archive_offset = cd_start
            .checked_sub(cd_offset)
            .ok_or_else(|| invalid("invalid central directory offset"))?;
        let mut pos = cd_offset
            .checked_add(archive_offset)
            .ok_or_else(|| invalid("central directory offset overflow"))?;
        let cd_limit = cd_start
            .checked_add(cd_size)
            .ok_or_else(|| invalid("central directory size overflow"))?;

        let mut names = HashSet::<Vec<u8>>::new();
        for _ in 0..count {
            let mut h = [0; 46];
            read_at(reader, pos, &mut h)?;
            if h[..4] != CENTRAL {
                return Err(invalid("invalid central directory entry signature"));
            }

            let name_len = u16::from_le_bytes([h[28], h[29]]) as usize;
            let extra_len = u16::from_le_bytes([h[30], h[31]]) as usize;
            let comment_len = u16::from_le_bytes([h[32], h[33]]) as usize;
            let variable_len = name_len
                .checked_add(extra_len)
                .and_then(|n| n.checked_add(comment_len))
                .ok_or_else(|| invalid("central directory record length overflow"))?;
            let record_end = pos
                .checked_add(46)
                .and_then(|n| n.checked_add(variable_len as u64))
                .ok_or_else(|| invalid("central directory record offset overflow"))?;

            if record_end > cd_limit || name_len == 0 {
                return Err(invalid("central directory record exceeds directory bounds"));
            }

            let mut name = vec![0; name_len];
            read_at(reader, pos + 46, &mut name)?;

            if !names.insert(name.clone()) {
                return Err(ApkError::DuplicateEntry {
                    name: String::from_utf8_lossy(&name).into_owned(),
                });
            }

            pos = record_end;
        }

        if pos > cd_limit {
            return Err(invalid("central directory exceeds declared size"));
        }

        // The declared count must consume the whole central directory. ZIP
        // permits one optional central-directory digital-signature record
        // after the file headers; no arbitrary trailing bytes are accepted.
        if pos < cd_limit {
            if cd_limit - pos < 6 {
                return Err(invalid("unexpected trailing bytes in central directory"));
            }

            let mut signature = [0; 6];
            read_at(reader, pos, &mut signature)?;
            if signature[..4] != DIGITAL_SIGNATURE {
                return Err(invalid("unexpected trailing bytes in central directory"));
            }

            let signature_len = u16::from_le_bytes([signature[4], signature[5]]) as u64;
            let signature_end = pos
                .checked_add(6)
                .and_then(|start| start.checked_add(signature_len))
                .ok_or_else(|| invalid("central directory signature length overflow"))?;
            if signature_end != cd_limit {
                return Err(invalid("invalid central directory digital signature length"));
            }
        }

        Ok(())
    })();

    // Keep the reader in the same initial position expected by the ZIP parser.
    let rewind = reader.seek(SeekFrom::Start(0)).map_err(|e| ApkError::Io {
        path: None,
        kind: e.kind(),
    });
    result?;
    rewind?;
    Ok(())
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
            .find(|&i| {
                tail[i..i + 4] == EOCD_SIGNATURE
                    && i + EOCD_MIN_LEN + u16::from_le_bytes([tail[i + 20], tail[i + 21]]) as usize
                        == tail.len()
            })
            .map(|i| u16::from_le_bytes([tail[i + 10], tail[i + 11]]));
    }
    reader.seek(SeekFrom::Start(0))?;
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

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
        assert_eq!(reason("C:filename"), Some(UnsafeEntryReason::DriveLetter));
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
    fn zip64_entry_count_sentinel_is_not_treated_as_a_classic_count() {
        assert!(!classic_count_exceeds_limit(u16::MAX, 512));
        assert!(classic_count_exceeds_limit(513, 512));
        assert!(!classic_count_exceeds_limit(512, 512));
    }

    fn zip_with_one_file() -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("entry.txt", zip::write::SimpleFileOptions::default())
            .expect("start file");
        writer.write_all(b"payload").expect("write file");
        writer.finish().expect("finish ZIP").into_inner()
    }

    fn eocd_offset(bytes: &[u8]) -> usize {
        assert!(bytes.len() >= EOCD_MIN_LEN);
        (0..=bytes.len() - EOCD_MIN_LEN)
            .rev()
            .find(|&i| {
                bytes[i..i + 4] == EOCD_SIGNATURE
                    && i + EOCD_MIN_LEN
                        + u16::from_le_bytes([bytes[i + 20], bytes[i + 21]]) as usize
                        == bytes.len()
            })
            .expect("EOCD")
    }

    fn extend_central_directory(bytes: &mut Vec<u8>, trailer: &[u8]) -> usize {
        let eocd = eocd_offset(bytes);
        let old_size =
            u32::from_le_bytes(bytes[eocd + 12..eocd + 16].try_into().expect("CD size"));
        bytes.splice(eocd..eocd, trailer.iter().copied());
        let new_eocd = eocd + trailer.len();
        let new_size = old_size
            .checked_add(u32::try_from(trailer.len()).expect("trailer length"))
            .expect("central directory size");
        bytes[new_eocd + 12..new_eocd + 16].copy_from_slice(&new_size.to_le_bytes());
        new_eocd
    }

    #[test]
    fn accepts_well_formed_central_directory_digital_signature() {
        let mut bytes = zip_with_one_file();
        extend_central_directory(&mut bytes, &[0x50, 0x4b, 0x05, 0x05, 0x00, 0x00]);

        let mut reader = Cursor::new(bytes);
        assert!(reject_duplicate_raw_names(&mut reader, 10).is_ok());
        assert_eq!(reader.position(), 0);
    }

    #[test]
    fn rejects_arbitrary_central_directory_trailing_data() {
        let mut bytes = zip_with_one_file();
        extend_central_directory(&mut bytes, &[0x99]);

        let mut reader = Cursor::new(bytes);
        assert!(matches!(
            reject_duplicate_raw_names(&mut reader, 10),
            Err(ApkError::InvalidArchive { reason })
                if reason.contains("trailing bytes")
        ));
        assert_eq!(reader.position(), 0);
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
