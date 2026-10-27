//! Bounded, read-only access to the entries of an APK (ZIP) archive.
//!
//! The central directory is located and scanned by this module *before* the
//! `zip` crate sees the archive, and the crate's own view is checked against
//! the scan afterwards. The rules, all enforced with checked arithmetic:
//!
//! - The end-of-central-directory (EOCD) record must be the last structure in
//!   the file: only its comment may follow it. If two records satisfy that
//!   rule (an EOCD-like signature inside a comment) the archive is rejected as
//!   ambiguous instead of guessing which one is real.
//! - When the classic EOCD holds ZIP64 sentinels (`0xFFFF` entries, `0xFFFFFFFF`
//!   size or offset) and a ZIP64 locator is present, the ZIP64 end record is
//!   authoritative. It is found by its position directly in front of the
//!   locator and its stored size, never by trusting the locator's offset as an
//!   absolute position: that offset is relative to the start of the archive
//!   proper, so archives with prepended data (self-extracting stubs) keep
//!   working.
//! - `max_entries` is applied to the authoritative entry count before any
//!   central-directory record is read or any count-sized buffer is allocated.
//! - Every central-directory record must lie inside the directory. After the
//!   last record only an optional digital-signature record may remain.
//! - Duplicate raw file names (compared as bytes, not as decoded text) are
//!   rejected before the `zip` crate builds its name-keyed index.
//! - After the `zip` crate has parsed the archive, its entry count and raw
//!   names must equal the scan exactly, so the two parsers cannot disagree
//!   about which directory was read.
//!
//! Multi-disk archives and data after the EOCD comment are not supported and
//! are rejected.

use super::error::{ApkError, UnsafeEntryReason};
use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use zip::ZipArchive;

/// Longest accepted entry name, in bytes.
pub(crate) const MAX_ENTRY_NAME_LEN: usize = 1024;

const EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const ZIP64_LOCATOR_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
const ZIP64_EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];
const CENTRAL_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
const DIGITAL_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x05];

const EOCD_LEN: usize = 22;
const MAX_COMMENT_LEN: usize = 65_535;
const LOCATOR_LEN: u64 = 20;
const ZIP64_EOCD_LEN: usize = 56;
const MAX_ZIP64_EXTENSIBLE_LEN: usize = 65_536;
const CENTRAL_HEADER_LEN: u64 = 46;
const NAME_DIAGNOSTIC_LEN: usize = 64;

const REASON_TRUNCATED: &str = "truncated ZIP directory";
const REASON_NO_EOCD: &str = "missing or invalid ZIP end-of-central-directory record";
const REASON_AMBIGUOUS_EOCD: &str = "ambiguous ZIP end-of-central-directory record";
const REASON_MULTI_DISK: &str = "multi-disk ZIP archives are not supported";
const REASON_ZIP64_RECORD: &str = "invalid ZIP64 end-of-central-directory record";
const REASON_ZIP64_LOCATOR: &str = "ZIP64 locator offset lies beyond its end record";
const REASON_CD_SIZE: &str = "central directory size exceeds the archive";
const REASON_CD_OFFSET: &str = "central directory offset lies beyond its start";
const REASON_CD_OVERFLOW: &str = "central directory offset overflows";
const REASON_CD_BOUNDS: &str = "central directory does not fit before its end record";
const REASON_CD_TOO_SMALL: &str = "central directory is too small for its entry count";
const REASON_RECORD_BOUNDS: &str = "central directory record exceeds the directory bounds";
const REASON_RECORD_SIGNATURE: &str = "invalid central directory record signature";
const REASON_CD_TRAILING: &str = "unexpected data after the last central directory record";
const REASON_DISAGREEMENT: &str = "ZIP library and directory scan disagree about the archive";

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

#[derive(Debug)]
pub(crate) struct ApkArchive<R: Read + Seek> {
    zip: ZipArchive<R>,
    entries: Vec<EntryInfo>,
    index: HashMap<String, usize>,
}

impl<R: Read + Seek> ApkArchive<R> {
    /// Open the archive and validate every entry name.
    ///
    /// The central directory is scanned first (see the module documentation);
    /// the `zip` crate is only given archives whose directory passed the scan,
    /// and its result must agree with the scan.
    pub(crate) fn open(mut reader: R, max_entries: usize) -> Result<Self, ApkError> {
        let raw_names = scan_central_directory(&mut reader, max_entries)?;

        let mut zip = ZipArchive::new(reader).map_err(|e| ApkError::InvalidArchive {
            reason: e.to_string(),
        })?;
        if zip.len() != raw_names.len() {
            return Err(invalid(REASON_DISAGREEMENT));
        }

        let mut entries = Vec::with_capacity(raw_names.len());
        let mut index = HashMap::with_capacity(raw_names.len());
        for (i, raw_name) in raw_names.iter().enumerate() {
            let file = zip.by_index_raw(i).map_err(|e| ApkError::InvalidArchive {
                reason: e.to_string(),
            })?;
            if file.name_raw() != raw_name.as_slice() {
                return Err(invalid(REASON_DISAGREEMENT));
            }
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

fn invalid(reason: &str) -> ApkError {
    ApkError::InvalidArchive {
        reason: reason.to_owned(),
    }
}

fn io_error(error: io::Error) -> ApkError {
    ApkError::Io {
        path: None,
        kind: error.kind(),
    }
}

/// Fill `buf` from `offset`. A short read is a truncated archive.
fn read_at<R: Read + Seek>(reader: &mut R, offset: u64, buf: &mut [u8]) -> Result<(), ApkError> {
    reader.seek(SeekFrom::Start(offset)).map_err(io_error)?;
    reader.read_exact(buf).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            invalid(REASON_TRUNCATED)
        } else {
            io_error(e)
        }
    })
}

// The helpers below index fixed-size records whose length the caller has
// already established, so the indices are in bounds by construction.
fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes([
        b[at],
        b[at + 1],
        b[at + 2],
        b[at + 3],
        b[at + 4],
        b[at + 5],
        b[at + 6],
        b[at + 7],
    ])
}

/// Where the central directory is and how many records it claims to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CentralDirectory {
    start: u64,
    size: u64,
    count: u64,
}

/// A parsed ZIP64 end-of-central-directory record.
struct Zip64 {
    count: u64,
    size: u64,
    offset: u64,
    /// Absolute position of the record in the file.
    position: u64,
    /// Bytes of data in front of the archive proper (`position` minus the
    /// locator's nominal offset).
    stub: u64,
}

/// Find the one EOCD record that ends exactly at the end of the file.
fn find_eocd(tail: &[u8]) -> Result<usize, ApkError> {
    let last = tail
        .len()
        .checked_sub(EOCD_LEN)
        .ok_or_else(|| invalid(REASON_NO_EOCD))?;
    let mut found = None;
    for i in (0..=last).rev() {
        let fits = tail[i..i + 4] == EOCD_SIGNATURE
            && i + EOCD_LEN + usize::from(le16(tail, i + 20)) == tail.len();
        if fits {
            if found.is_some() {
                return Err(invalid(REASON_AMBIGUOUS_EOCD));
            }
            found = Some(i);
        }
    }
    found.ok_or_else(|| invalid(REASON_NO_EOCD))
}

/// Resolve the ZIP64 end record for an EOCD at `eocd_pos`, if a locator
/// precedes it. `Ok(None)` means there is no ZIP64 locator.
fn read_zip64<R: Read + Seek>(reader: &mut R, eocd_pos: u64) -> Result<Option<Zip64>, ApkError> {
    let Some(locator_pos) = eocd_pos.checked_sub(LOCATOR_LEN) else {
        return Ok(None);
    };
    let mut locator = [0u8; LOCATOR_LEN as usize];
    read_at(reader, locator_pos, &mut locator)?;
    if locator[..4] != ZIP64_LOCATOR_SIGNATURE {
        return Ok(None);
    }
    if le32(&locator, 4) != 0 || le32(&locator, 16) > 1 {
        return Err(invalid(REASON_MULTI_DISK));
    }
    // Relative to the start of the archive proper, not to the start of the file.
    let nominal = le64(&locator, 8);

    // The record sits directly in front of its locator, so it is found by that
    // adjacency (position + 12 + stored size == locator position) inside a
    // bounded window rather than by trusting the locator's offset.
    let window_len = locator_pos.min((ZIP64_EOCD_LEN + MAX_ZIP64_EXTENSIBLE_LEN) as u64);
    if window_len < ZIP64_EOCD_LEN as u64 {
        return Err(invalid(REASON_ZIP64_RECORD));
    }
    let window_start = locator_pos - window_len;
    let mut window = vec![0u8; window_len as usize];
    read_at(reader, window_start, &mut window)?;

    let mut found = None;
    for i in (0..=window.len() - ZIP64_EOCD_LEN).rev() {
        if window[i..i + 4] != ZIP64_EOCD_SIGNATURE {
            continue;
        }
        let record_size = le64(&window, i + 4);
        let adjacent = (i as u64)
            .checked_add(12)
            .and_then(|n| n.checked_add(record_size))
            == Some(window_len);
        if adjacent && record_size >= 44 {
            if found.is_some() {
                return Err(invalid(REASON_ZIP64_RECORD));
            }
            found = Some(i);
        }
    }
    let i = found.ok_or_else(|| invalid(REASON_ZIP64_RECORD))?;
    let record = &window[i..i + ZIP64_EOCD_LEN];
    let position = window_start + i as u64;

    if le32(record, 16) != le32(record, 20) || le64(record, 24) != le64(record, 32) {
        return Err(invalid(REASON_MULTI_DISK));
    }
    let stub = position
        .checked_sub(nominal)
        .ok_or_else(|| invalid(REASON_ZIP64_LOCATOR))?;
    Ok(Some(Zip64 {
        count: le64(record, 32),
        size: le64(record, 40),
        offset: le64(record, 48),
        position,
        stub,
    }))
}

/// Locate the central directory from the end records.
fn locate_central_directory<R: Read + Seek>(reader: &mut R) -> Result<CentralDirectory, ApkError> {
    let len = reader.seek(SeekFrom::End(0)).map_err(io_error)?;
    if len < EOCD_LEN as u64 {
        return Err(invalid(REASON_NO_EOCD));
    }
    let tail_len = len.min((EOCD_LEN + MAX_COMMENT_LEN) as u64);
    let tail_start = len - tail_len;
    let mut tail = vec![0u8; tail_len as usize];
    read_at(reader, tail_start, &mut tail)?;

    let eocd_index = find_eocd(&tail)?;
    let eocd = &tail[eocd_index..eocd_index + EOCD_LEN];
    let eocd_pos = tail_start + eocd_index as u64;

    let entries_on_disk = le16(eocd, 8);
    let entries_total = le16(eocd, 10);
    if le16(eocd, 4) != le16(eocd, 6) || entries_on_disk != entries_total {
        return Err(invalid(REASON_MULTI_DISK));
    }
    let size = u64::from(le32(eocd, 12));
    let offset = u64::from(le32(eocd, 16));

    let sentinel = entries_total == u16::MAX
        || size == u64::from(u32::MAX)
        || offset == u64::from(u32::MAX);
    let zip64 = if sentinel {
        read_zip64(reader, eocd_pos)?
    } else {
        None
    };

    match zip64 {
        Some(zip64) => {
            let start = zip64
                .offset
                .checked_add(zip64.stub)
                .ok_or_else(|| invalid(REASON_CD_OVERFLOW))?;
            let fits = start
                .checked_add(zip64.size)
                .is_some_and(|end| end <= zip64.position);
            if !fits {
                return Err(invalid(REASON_CD_BOUNDS));
            }
            Ok(CentralDirectory {
                start,
                size: zip64.size,
                count: zip64.count,
            })
        }
        None => {
            let start = eocd_pos
                .checked_sub(size)
                .ok_or_else(|| invalid(REASON_CD_SIZE))?;
            if offset > start {
                return Err(invalid(REASON_CD_OFFSET));
            }
            Ok(CentralDirectory {
                start,
                size,
                count: u64::from(entries_total),
            })
        }
    }
}

/// Scan the central directory and return the raw file names in archive order.
///
/// Enforces `max_entries` on the authoritative count, bounds-checks every
/// record, and rejects duplicate raw names. The reader is rewound to offset
/// zero whether the scan succeeds or fails.
fn scan_central_directory<R: Read + Seek>(
    reader: &mut R,
    max_entries: usize,
) -> Result<Vec<Vec<u8>>, ApkError> {
    let result = scan_records(reader, max_entries);
    let rewind = reader.seek(SeekFrom::Start(0)).map_err(io_error);
    let names = result?;
    rewind?;
    Ok(names)
}

fn scan_records<R: Read + Seek>(
    reader: &mut R,
    max_entries: usize,
) -> Result<Vec<Vec<u8>>, ApkError> {
    let dir = locate_central_directory(reader)?;
    if dir.count > max_entries as u64 {
        return Err(ApkError::TooManyEntries {
            count: usize::try_from(dir.count).unwrap_or(usize::MAX),
            max: max_entries,
        });
    }
    let min_size = dir
        .count
        .checked_mul(CENTRAL_HEADER_LEN)
        .ok_or_else(|| invalid(REASON_CD_TOO_SMALL))?;
    if min_size > dir.size {
        return Err(invalid(REASON_CD_TOO_SMALL));
    }
    let cd_end = dir
        .start
        .checked_add(dir.size)
        .ok_or_else(|| invalid(REASON_CD_OVERFLOW))?;

    let count = dir.count as usize;
    let mut names: Vec<Vec<u8>> = Vec::with_capacity(count);
    let mut pos = dir.start;
    for _ in 0..count {
        let header_end = pos
            .checked_add(CENTRAL_HEADER_LEN)
            .filter(|&end| end <= cd_end)
            .ok_or_else(|| invalid(REASON_RECORD_BOUNDS))?;
        let mut header = [0u8; CENTRAL_HEADER_LEN as usize];
        read_at(reader, pos, &mut header)?;
        if header[..4] != CENTRAL_SIGNATURE {
            return Err(invalid(REASON_RECORD_SIGNATURE));
        }
        let name_len = u64::from(le16(&header, 28));
        let variable_len = name_len + u64::from(le16(&header, 30)) + u64::from(le16(&header, 32));
        let record_end = header_end
            .checked_add(variable_len)
            .filter(|&end| end <= cd_end)
            .ok_or_else(|| invalid(REASON_RECORD_BOUNDS))?;

        if name_len == 0 {
            return Err(ApkError::UnsafeEntryName {
                name: String::new(),
                reason: UnsafeEntryReason::Empty,
            });
        }
        if name_len > MAX_ENTRY_NAME_LEN as u64 {
            let shown = usize::try_from(name_len)
                .unwrap_or(usize::MAX)
                .min(NAME_DIAGNOSTIC_LEN);
            let mut prefix = vec![0u8; shown];
            read_at(reader, header_end, &mut prefix)?;
            return Err(ApkError::UnsafeEntryName {
                name: String::from_utf8_lossy(&prefix).into_owned(),
                reason: UnsafeEntryReason::TooLong,
            });
        }
        let mut name = vec![0u8; name_len as usize];
        read_at(reader, header_end, &mut name)?;
        names.push(name);
        pos = record_end;
    }

    // Only a digital-signature record may follow the last record.
    let remaining = cd_end - pos;
    if remaining != 0 {
        let mut signature = [0u8; 6];
        if remaining < 6 {
            return Err(invalid(REASON_CD_TRAILING));
        }
        read_at(reader, pos, &mut signature)?;
        if signature[..4] != DIGITAL_SIGNATURE
            || 6 + u64::from(le16(&signature, 4)) != remaining
        {
            return Err(invalid(REASON_CD_TRAILING));
        }
    }

    let mut order: Vec<usize> = (0..names.len()).collect();
    order.sort_unstable_by(|&a, &b| names[a].cmp(&names[b]));
    for pair in order.windows(2) {
        if names[pair[0]] == names[pair[1]] {
            return Err(ApkError::DuplicateEntry {
                name: String::from_utf8_lossy(&names[pair[0]]).into_owned(),
            });
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apk::testutil::{build_zip, patch_name};
    use std::io::Cursor;
    use zip::CompressionMethod;

    fn zip_of(names: &[&str]) -> Vec<u8> {
        let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (*n, &b"x"[..])).collect();
        build_zip(&entries, CompressionMethod::Stored)
    }

    fn three() -> Vec<u8> {
        zip_of(&["a.txt", "b.txt", "c.txt"])
    }

    fn open(bytes: &[u8], max_entries: usize) -> Result<ApkArchive<Cursor<Vec<u8>>>, ApkError> {
        ApkArchive::open(Cursor::new(bytes.to_vec()), max_entries)
    }

    fn open_err(bytes: &[u8], max_entries: usize) -> ApkError {
        open(bytes, max_entries).expect_err("archive should be rejected")
    }

    fn names<R: Read + Seek>(archive: &ApkArchive<R>) -> Vec<&str> {
        archive.entries().iter().map(|e| e.name.as_str()).collect()
    }

    fn put(bytes: &mut [u8], at: usize, value: &[u8]) {
        bytes[at..at + value.len()].copy_from_slice(value);
    }

    /// A comment-less, stub-less zip ends with its 22-byte EOCD.
    fn eocd_of(plain: &[u8]) -> usize {
        plain.len() - EOCD_LEN
    }

    fn with_comment(plain: &[u8], comment: &[u8]) -> Vec<u8> {
        let mut out = plain.to_vec();
        let at = out.len() - 2;
        put(&mut out, at, &(comment.len() as u16).to_le_bytes());
        out.extend_from_slice(comment);
        out
    }

    fn eocd_record(entries: u16, size: u32, offset: u32) -> Vec<u8> {
        let mut out = EOCD_SIGNATURE.to_vec();
        out.extend_from_slice(&0u16.to_le_bytes()); // this disk
        out.extend_from_slice(&0u16.to_le_bytes()); // directory disk
        out.extend_from_slice(&entries.to_le_bytes());
        out.extend_from_slice(&entries.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment length
        out
    }

    fn sentinel_eocd() -> Vec<u8> {
        eocd_record(u16::MAX, u32::MAX, u32::MAX)
    }

    fn locator(nominal_offset: u64) -> Vec<u8> {
        let mut out = ZIP64_LOCATOR_SIGNATURE.to_vec();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&nominal_offset.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out
    }

    /// Rewrite a plain zip so it ends with ZIP64 records and a sentinel EOCD.
    /// Layout of the result: ... directory | ZIP64 record (56) | locator (20) | EOCD (22).
    fn zip64(plain: &[u8]) -> Vec<u8> {
        let eocd = eocd_of(plain);
        let count = u64::from(le16(plain, eocd + 10));
        let size = u64::from(le32(plain, eocd + 12));
        let offset = u64::from(le32(plain, eocd + 16));
        let mut out = plain[..eocd].to_vec();
        let record_pos = out.len() as u64;
        out.extend_from_slice(&ZIP64_EOCD_SIGNATURE);
        out.extend_from_slice(&44u64.to_le_bytes());
        out.extend_from_slice(&45u16.to_le_bytes()); // version made by
        out.extend_from_slice(&45u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u32.to_le_bytes()); // this disk
        out.extend_from_slice(&0u32.to_le_bytes()); // directory disk
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&locator(record_pos));
        out.extend_from_slice(&sentinel_eocd());
        out
    }

    // ---- ordinary archives ----

    #[test]
    fn plain_archives_open_and_keep_entry_order() {
        let archive = open(&three(), 100).expect("open");
        assert_eq!(names(&archive), vec!["a.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn the_scan_returns_raw_names_in_archive_order() {
        let mut cursor = Cursor::new(three());
        let raw = scan_central_directory(&mut cursor, 100).expect("scan");
        assert_eq!(
            raw,
            vec![b"a.txt".to_vec(), b"b.txt".to_vec(), b"c.txt".to_vec()]
        );
    }

    #[test]
    fn empty_archives_and_comments_are_valid() {
        let empty = build_zip(&[], CompressionMethod::Stored);
        assert!(open(&empty, 100).expect("open").entries().is_empty());

        let commented = with_comment(&three(), b"hello");
        let archive = open(&commented, 100).expect("open");
        assert_eq!(names(&archive).len(), 3);
    }

    #[test]
    fn the_entry_limit_applies_to_ordinary_archives() {
        assert_eq!(
            open_err(&three(), 2),
            ApkError::TooManyEntries { count: 3, max: 2 }
        );
        assert!(open(&three(), 3).is_ok());
    }

    #[test]
    fn plain_archives_with_prepended_data_are_accepted() {
        let stub = vec![b'S'; 37];
        let plain = three();
        let bytes = [stub.as_slice(), plain.as_slice()].concat();
        let archive = open(&bytes, 100).expect("open");
        assert_eq!(names(&archive), vec!["a.txt", "b.txt", "c.txt"]);
    }

    // ---- duplicate raw names ----

    #[test]
    fn duplicate_raw_names_are_rejected_before_the_zip_index_is_built() {
        let mut bytes = zip_of(&["aaa.txt", "bbb.txt", "ccc.txt"]);
        assert_eq!(patch_name(&mut bytes, b"bbb.txt", b"aaa.txt"), 2);
        assert_eq!(
            open_err(&bytes, 100),
            ApkError::DuplicateEntry {
                name: "aaa.txt".to_owned()
            }
        );
    }

    #[test]
    fn duplicate_non_utf8_names_are_reported_without_panicking() {
        let mut bytes = zip_of(&["aaaa", "bbbb", "cccc"]);
        let raw = [0xff, 0xfe, 0x41, 0x42];
        patch_name(&mut bytes, b"aaaa", &raw);
        patch_name(&mut bytes, b"bbbb", &raw);
        assert_eq!(
            open_err(&bytes, 100),
            ApkError::DuplicateEntry {
                name: String::from_utf8_lossy(&raw).into_owned()
            }
        );
    }

    #[test]
    fn distinct_non_utf8_names_are_kept_distinct() {
        let mut bytes = zip_of(&["aaaa", "bbbb"]);
        patch_name(&mut bytes, b"aaaa", &[0xff, 0xfe, 0x41, 0x42]);
        patch_name(&mut bytes, b"bbbb", &[0xfe, 0xff, 0x41, 0x42]);
        let archive = open(&bytes, 100).expect("open");
        assert_eq!(archive.entries().len(), 2);
        assert_ne!(archive.entries()[0].name, archive.entries()[1].name);
    }

    // ---- ZIP64 ----

    #[test]
    fn zip64_sentinel_counts_are_resolved_before_the_limit_applies() {
        // The classic EOCD says 0xFFFF entries; the real count is 3. A limit
        // below 65535 must not reject this archive.
        let archive = open(&zip64(&three()), 100).expect("open");
        assert_eq!(names(&archive), vec!["a.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn the_zip64_count_is_the_one_checked_against_the_limit() {
        assert_eq!(
            open_err(&zip64(&three()), 2),
            ApkError::TooManyEntries { count: 3, max: 2 }
        );
    }

    #[test]
    fn zip64_locator_offsets_are_relative_to_the_archive_start() {
        // With a prepended stub the locator's offset no longer equals the
        // record's position in the file; treating it as absolute fails.
        let stub = vec![b'S'; 37];
        let inner = zip64(&three());
        let bytes = [stub.as_slice(), inner.as_slice()].concat();
        let archive = open(&bytes, 100).expect("open");
        assert_eq!(names(&archive), vec!["a.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn zip64_entry_counts_beyond_the_limit_are_rejected_before_any_allocation() {
        let plain = three();
        let record = eocd_of(&plain);
        let mut bytes = zip64(&plain);
        put(&mut bytes, record + 24, &u64::MAX.to_le_bytes());
        put(&mut bytes, record + 32, &u64::MAX.to_le_bytes());
        assert_eq!(
            open_err(&bytes, 100),
            ApkError::TooManyEntries {
                count: usize::try_from(u64::MAX).unwrap_or(usize::MAX),
                max: 100
            }
        );
    }

    #[test]
    fn zip64_offsets_that_overflow_are_rejected() {
        let plain = three();
        let record = eocd_of(&plain);

        let mut offset = zip64(&plain);
        put(&mut offset, record + 48, &u64::MAX.to_le_bytes());
        assert_eq!(open_err(&offset, 100), invalid(REASON_CD_BOUNDS));

        let mut size = zip64(&plain);
        put(&mut size, record + 40, &u64::MAX.to_le_bytes());
        assert_eq!(open_err(&size, 100), invalid(REASON_CD_BOUNDS));

        // With a stub in front the addition itself overflows.
        let stub = vec![b'S'; 10];
        let inner = zip64(&plain);
        let mut stubbed = [stub.as_slice(), inner.as_slice()].concat();
        put(&mut stubbed, 10 + record + 48, &u64::MAX.to_le_bytes());
        assert_eq!(open_err(&stubbed, 100), invalid(REASON_CD_OVERFLOW));
    }

    #[test]
    fn malformed_zip64_records_are_rejected() {
        let plain = three();
        let record = eocd_of(&plain);
        let good = zip64(&plain);

        let mut bad_signature = good.clone();
        put(&mut bad_signature, record, b"XXXX");
        assert_eq!(open_err(&bad_signature, 100), invalid(REASON_ZIP64_RECORD));

        // A stored size that does not end at the locator breaks adjacency.
        let mut bad_size = good.clone();
        put(&mut bad_size, record + 4, &1000u64.to_le_bytes());
        assert_eq!(open_err(&bad_size, 100), invalid(REASON_ZIP64_RECORD));

        // The locator points beyond the record it describes.
        let mut bad_nominal = good.clone();
        put(&mut bad_nominal, record + 56 + 8, &u64::MAX.to_le_bytes());
        assert_eq!(open_err(&bad_nominal, 100), invalid(REASON_ZIP64_LOCATOR));

        // Multi-disk claims in the locator and in the record.
        let mut locator_disks = good.clone();
        put(&mut locator_disks, record + 56 + 16, &2u32.to_le_bytes());
        assert_eq!(open_err(&locator_disks, 100), invalid(REASON_MULTI_DISK));
        let mut record_disks = good.clone();
        put(&mut record_disks, record + 16, &1u32.to_le_bytes());
        assert_eq!(open_err(&record_disks, 100), invalid(REASON_MULTI_DISK));

        // Without a recognizable locator the sentinels are taken literally,
        // and a 4 GiB directory cannot fit.
        let mut no_locator = good.clone();
        put(&mut no_locator, record + 56, b"XXXX");
        assert_eq!(open_err(&no_locator, 100), invalid(REASON_CD_SIZE));
    }

    #[test]
    fn a_zip64_locator_with_no_room_for_its_record_is_rejected() {
        let mut tiny = locator(0);
        tiny.extend_from_slice(&sentinel_eocd());
        assert_eq!(open_err(&tiny, 100), invalid(REASON_ZIP64_RECORD));
    }

    // ---- EOCD selection ----

    #[test]
    fn truncated_end_records_are_rejected() {
        let bytes = three();
        for cut in 1..EOCD_LEN {
            assert_eq!(
                open_err(&bytes[..bytes.len() - cut], 100),
                invalid(REASON_NO_EOCD),
                "cut {cut}"
            );
        }
        assert_eq!(open_err(&[], 100), invalid(REASON_NO_EOCD));
        assert_eq!(open_err(&bytes[..21], 100), invalid(REASON_NO_EOCD));

        let z64 = zip64(&three());
        for cut in [1, 10, 21, 22, 30, 60, 90] {
            assert_eq!(
                open_err(&z64[..z64.len() - cut], 100),
                invalid(REASON_NO_EOCD),
                "zip64 cut {cut}"
            );
        }
    }

    #[test]
    fn an_eocd_lookalike_in_a_comment_cannot_replace_the_real_record() {
        // A decoy that also ends exactly at the end of the file and claims a
        // single entry. Picking it would slip past a limit of 2 (real count 3).
        let decoy = eocd_record(1, 0, 0);
        let bytes = with_comment(&three(), &decoy);
        assert_eq!(open_err(&bytes, 2), invalid(REASON_AMBIGUOUS_EOCD));
        assert_eq!(open_err(&bytes, 100), invalid(REASON_AMBIGUOUS_EOCD));
    }

    #[test]
    fn a_signature_in_a_comment_that_does_not_fit_is_ignored() {
        let mut comment = EOCD_SIGNATURE.to_vec();
        comment.extend_from_slice(&[0xff; 30]);
        let bytes = with_comment(&three(), &comment);
        let archive = open(&bytes, 3).expect("open");
        assert_eq!(names(&archive).len(), 3);
        assert_eq!(
            open_err(&bytes, 2),
            ApkError::TooManyEntries { count: 3, max: 2 }
        );
    }

    // ---- central directory bounds ----

    #[test]
    fn central_directory_records_are_bounds_checked() {
        let plain = three();
        let eocd = eocd_of(&plain);
        let cd_start = le32(&plain, eocd + 16) as usize;

        let mut name_len = plain.clone();
        put(&mut name_len, cd_start + 28, &0xffffu16.to_le_bytes());
        assert_eq!(open_err(&name_len, 100), invalid(REASON_RECORD_BOUNDS));

        let mut signature = plain.clone();
        put(&mut signature, cd_start, b"XXXX");
        assert_eq!(open_err(&signature, 100), invalid(REASON_RECORD_SIGNATURE));

        // The scan starts inside the first record.
        let size = le32(&plain, eocd + 12);
        let mut shifted = plain.clone();
        put(&mut shifted, eocd + 12, &(size - 10).to_le_bytes());
        assert_eq!(open_err(&shifted, 100), invalid(REASON_RECORD_SIGNATURE));
    }

    #[test]
    fn inconsistent_directory_metadata_is_rejected() {
        let plain = three();
        let eocd = eocd_of(&plain);

        // More records claimed than the directory can hold.
        let mut too_many = plain.clone();
        put(&mut too_many, eocd + 8, &1000u16.to_le_bytes());
        put(&mut too_many, eocd + 10, &1000u16.to_le_bytes());
        assert_eq!(open_err(&too_many, 100_000), invalid(REASON_CD_TOO_SMALL));

        // Fewer records claimed than the directory holds.
        let mut too_few = plain.clone();
        put(&mut too_few, eocd + 8, &2u16.to_le_bytes());
        put(&mut too_few, eocd + 10, &2u16.to_le_bytes());
        assert_eq!(open_err(&too_few, 100), invalid(REASON_CD_TRAILING));

        // Directory size larger than the whole archive.
        let mut size = plain.clone();
        put(&mut size, eocd + 12, &0x7fff_ffffu32.to_le_bytes());
        assert_eq!(open_err(&size, 100), invalid(REASON_CD_SIZE));

        // Directory offset beyond where the directory starts.
        let mut offset = plain.clone();
        put(&mut offset, eocd + 16, &0x7fff_ffffu32.to_le_bytes());
        assert_eq!(open_err(&offset, 100), invalid(REASON_CD_OFFSET));

        // Disagreeing entry counts mean a multi-disk or damaged archive.
        let mut counts = plain.clone();
        put(&mut counts, eocd + 8, &2u16.to_le_bytes());
        assert_eq!(open_err(&counts, 100), invalid(REASON_MULTI_DISK));
    }

    #[test]
    fn only_a_digital_signature_may_follow_the_last_record() {
        let plain = three();
        let eocd = eocd_of(&plain);
        let build = |extra: &[u8]| {
            let mut bytes = plain[..eocd].to_vec();
            bytes.extend_from_slice(extra);
            let mut end = plain[eocd..].to_vec();
            let size = le32(&end, 12) + extra.len() as u32;
            put(&mut end, 12, &size.to_le_bytes());
            bytes.extend_from_slice(&end);
            bytes
        };

        let mut signature = DIGITAL_SIGNATURE.to_vec();
        signature.extend_from_slice(&4u16.to_le_bytes());
        signature.extend_from_slice(&[0u8; 4]);
        let archive = open(&build(&signature), 100).expect("signature is legal");
        assert_eq!(names(&archive).len(), 3);

        assert_eq!(
            open_err(&build(&[0u8; 10]), 100),
            invalid(REASON_CD_TRAILING)
        );
        assert_eq!(open_err(&build(&[0u8; 3]), 100), invalid(REASON_CD_TRAILING));
    }

    // ---- reader position ----

    #[test]
    fn scanning_restores_the_reader_position_on_success_and_failure() {
        let mut ok = Cursor::new(three());
        assert!(scan_central_directory(&mut ok, 100).is_ok());
        assert_eq!(ok.stream_position().expect("position"), 0);

        let mut garbage = Cursor::new(b"not a zip, but long enough to pass the length check".to_vec());
        assert_eq!(
            scan_central_directory(&mut garbage, 100),
            Err(invalid(REASON_NO_EOCD))
        );
        assert_eq!(garbage.stream_position().expect("position"), 0);

        let plain = three();
        let cd_start = le32(&plain, eocd_of(&plain) + 16) as usize;
        let mut broken = plain.clone();
        put(&mut broken, cd_start, b"XXXX");
        let mut inside = Cursor::new(broken);
        assert_eq!(
            scan_central_directory(&mut inside, 100),
            Err(invalid(REASON_RECORD_SIGNATURE))
        );
        assert_eq!(inside.stream_position().expect("position"), 0);
    }

    // ---- entry names ----

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
