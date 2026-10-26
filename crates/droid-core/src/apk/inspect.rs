//! The public APK inspection entry points and report types.

use super::archive::{ApkArchive, ReadBudget};
use super::axml::AxmlLimits;
use super::error::ApkError;
use super::manifest::{ManifestInfo, parse_manifest};
use crate::dex::{DexError, DexFile, DexLimits};
use std::fs::File;
use std::io::{self, BufReader, Cursor, Read, Seek};
use std::path::Path;

/// Name of the manifest entry every APK must contain.
pub const MANIFEST_ENTRY: &str = "AndroidManifest.xml";

/// Resource limits applied while inspecting an APK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApkLimits {
    /// Largest accepted APK file, in bytes.
    pub max_apk_size: u64,
    /// Largest accepted number of archive entries.
    pub max_entries: usize,
    /// Largest accepted uncompressed `AndroidManifest.xml`, in bytes.
    pub max_manifest_size: u64,
    /// Largest accepted uncompressed DEX file, in bytes.
    pub max_dex_size: u64,
    /// Cap on the total uncompressed bytes read from the archive.
    pub max_total_read: u64,
    /// Limits for parsing the binary manifest.
    pub axml: AxmlLimits,
}

impl Default for ApkLimits {
    fn default() -> Self {
        Self {
            max_apk_size: 512 * 1024 * 1024,
            max_entries: 100_000,
            max_manifest_size: 8 * 1024 * 1024,
            max_dex_size: 64 * 1024 * 1024,
            max_total_read: 256 * 1024 * 1024,
            axml: AxmlLimits::default(),
        }
    }
}

/// Counts taken from a successfully parsed DEX file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexSummary {
    /// The DEX format version, for example `035`.
    pub version: String,
    pub string_count: usize,
    pub type_count: usize,
    pub field_count: usize,
    pub method_count: usize,
    pub class_count: usize,
}

/// Why one DEX entry could not be summarized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DexEntryError {
    /// The entry could not be read from the archive (too large, corrupt, ...).
    Unreadable(ApkError),
    /// The entry was read but is not a valid DEX file.
    Invalid(DexError),
}

/// The result for one `classes*.dex` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexEntryReport {
    pub entry_name: String,
    /// Uncompressed size declared by the archive.
    pub declared_size: u64,
    pub outcome: Result<DexSummary, DexEntryError>,
}

/// A native library found under `lib/<abi>/`. Only names are inspected; the
/// library contents are never read or loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeLibrary {
    pub abi: String,
    pub file_name: String,
    /// Uncompressed size declared by the archive.
    pub size: u64,
}

/// Something worth knowing that is not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApkWarning {
    /// The APK contains no `classes*.dex` files.
    NoDexFiles,
}

/// What inspecting an APK found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApkReport {
    pub entry_count: usize,
    pub manifest: ManifestInfo,
    /// DEX files in load order: `classes.dex`, `classes2.dex`, `classes3.dex`, ...
    pub dex_files: Vec<DexEntryReport>,
    /// Native libraries sorted by ABI, then file name.
    pub native_libraries: Vec<NativeLibrary>,
    pub warnings: Vec<ApkWarning>,
}

impl ApkReport {
    /// DEX entries that could not be read or parsed.
    pub fn dex_failures(&self) -> impl Iterator<Item = &DexEntryReport> {
        self.dex_files.iter().filter(|d| d.outcome.is_err())
    }

    /// `true` if every DEX entry parsed (vacuously true with no DEX files).
    pub fn all_dex_valid(&self) -> bool {
        self.dex_files.iter().all(|d| d.outcome.is_ok())
    }
}

/// Inspect the APK at `path`. The file is streamed, not loaded whole.
pub fn inspect_file(path: impl AsRef<Path>, limits: &ApkLimits) -> Result<ApkReport, ApkError> {
    let path = path.as_ref();
    let io_err = |e: io::Error| ApkError::Io {
        path: Some(path.to_path_buf()),
        kind: e.kind(),
    };
    let file = File::open(path).map_err(io_err)?;
    let size = file.metadata().map_err(io_err)?.len();
    if size > limits.max_apk_size {
        return Err(ApkError::FileTooLarge {
            size,
            max: limits.max_apk_size,
        });
    }
    inspect_reader(BufReader::new(file), limits)
}

/// Inspect an APK held in memory.
pub fn inspect_bytes(bytes: &[u8], limits: &ApkLimits) -> Result<ApkReport, ApkError> {
    let size = bytes.len() as u64;
    if size > limits.max_apk_size {
        return Err(ApkError::FileTooLarge {
            size,
            max: limits.max_apk_size,
        });
    }
    inspect_reader(Cursor::new(bytes), limits)
}

/// Inspect an APK from any seekable reader. The caller is responsible for
/// bounding the reader's size (see [`inspect_file`] and [`inspect_bytes`]).
pub fn inspect_reader<R: Read + Seek>(
    reader: R,
    limits: &ApkLimits,
) -> Result<ApkReport, ApkError> {
    let mut archive = ApkArchive::open(reader, limits.max_entries)?;
    let mut budget = ReadBudget::new(limits.max_total_read);

    if !archive.contains(MANIFEST_ENTRY) {
        return Err(ApkError::MissingEntry {
            name: MANIFEST_ENTRY,
        });
    }
    let manifest_bytes =
        archive.read_entry(MANIFEST_ENTRY, limits.max_manifest_size, &mut budget)?;
    let manifest = parse_manifest(&manifest_bytes, &limits.axml)?;

    let mut native_libraries: Vec<NativeLibrary> = archive
        .entries()
        .iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| native_library(&e.name, e.size))
        .collect();
    native_libraries.sort_by(|a, b| (&a.abi, &a.file_name).cmp(&(&b.abi, &b.file_name)));

    let mut dex_entries: Vec<(u32, String, u64)> = archive
        .entries()
        .iter()
        .filter(|e| !e.is_dir)
        .filter_map(|e| dex_index(&e.name).map(|i| (i, e.name.clone(), e.size)))
        .collect();
    dex_entries.sort_by_key(|(index, _, _)| *index);

    let dex_limits = DexLimits {
        max_file_size: usize::try_from(limits.max_dex_size).unwrap_or(usize::MAX),
    };
    let mut dex_files = Vec::with_capacity(dex_entries.len());
    for (_, name, declared_size) in dex_entries {
        let outcome = match archive.read_entry(&name, limits.max_dex_size, &mut budget) {
            Ok(bytes) => match DexFile::parse(&bytes, &dex_limits) {
                Ok(dex) => Ok(summarize(&dex)),
                Err(e) => Err(DexEntryError::Invalid(e)),
            },
            Err(ApkError::TotalSizeExceeded { max }) => {
                return Err(ApkError::TotalSizeExceeded { max });
            }
            Err(e) => Err(DexEntryError::Unreadable(e)),
        };
        dex_files.push(DexEntryReport {
            entry_name: name,
            declared_size,
            outcome,
        });
    }

    let mut warnings = Vec::new();
    if dex_files.is_empty() {
        warnings.push(ApkWarning::NoDexFiles);
    }

    Ok(ApkReport {
        entry_count: archive.entries().len(),
        manifest,
        dex_files,
        native_libraries,
        warnings,
    })
}

fn summarize(dex: &DexFile<'_>) -> DexSummary {
    DexSummary {
        version: String::from_utf8_lossy(&dex.header().version).into_owned(),
        string_count: dex.string_count(),
        type_count: dex.type_count(),
        field_count: dex.fields().len(),
        method_count: dex.methods().len(),
        class_count: dex.class_defs().len(),
    }
}

/// Position of a top-level `classes.dex` / `classesN.dex` file in load order:
/// `classes.dex` is 1, `classes2.dex` is 2, and so on. Other names, including
/// `classes1.dex`, `classes02.dex` and nested paths, are not DEX entries.
fn dex_index(name: &str) -> Option<u32> {
    let middle = name.strip_prefix("classes")?.strip_suffix(".dex")?;
    if middle.is_empty() {
        return Some(1);
    }
    if middle.starts_with('0') || !middle.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = middle.parse().ok()?;
    (n >= 2).then_some(n)
}

/// Recognize `lib/<abi>/<file>.so` and nothing deeper.
fn native_library(name: &str, size: u64) -> Option<NativeLibrary> {
    let rest = name.strip_prefix("lib/")?;
    let (abi, file) = rest.split_once('/')?;
    if abi.is_empty() || file.is_empty() || file.contains('/') || !file.ends_with(".so") {
        return None;
    }
    Some(NativeLibrary {
        abi: abi.to_owned(),
        file_name: file.to_owned(),
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apk::error::UnsafeEntryReason;
    use crate::apk::manifest::ManifestValue;
    use crate::apk::testutil::{
        TestValue, android, build_axml, build_zip, end, minimal_manifest_axml, patch_name, plain,
        sample_dex, sample_manifest_axml, start,
    };
    use zip::CompressionMethod;

    fn e<'a>(name: &'a str, data: &'a [u8]) -> (&'a str, &'a [u8]) {
        (name, data)
    }

    fn stored(entries: &[(&str, &[u8])]) -> Vec<u8> {
        build_zip(entries, CompressionMethod::Stored)
    }

    fn inspect(bytes: &[u8]) -> Result<ApkReport, ApkError> {
        inspect_bytes(bytes, &ApkLimits::default())
    }

    fn minimal_apk() -> Vec<u8> {
        let manifest = minimal_manifest_axml("com.example.app");
        let dex = sample_dex();
        stored(&[e(MANIFEST_ENTRY, &manifest), e("classes.dex", &dex)])
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("droid-core-apk-{}-{tag}.apk", std::process::id()))
    }

    #[test]
    fn inspects_a_minimal_apk() {
        let report = inspect(&minimal_apk()).expect("inspect");
        assert_eq!(report.entry_count, 2);
        assert_eq!(report.manifest.package, "com.example.app");
        assert!(report.warnings.is_empty());
        assert!(report.native_libraries.is_empty());
        assert!(report.all_dex_valid());
        assert_eq!(report.dex_files.len(), 1);
        assert_eq!(report.dex_files[0].entry_name, "classes.dex");
        let summary = report.dex_files[0].outcome.as_ref().expect("dex summary");
        assert_eq!(summary.version, "035");
        assert_eq!(summary.class_count, 1);
        assert_eq!(summary.method_count, 1);
    }

    #[test]
    fn deflated_entries_are_read_too() {
        let manifest = sample_manifest_axml(true);
        let dex = sample_dex();
        let bytes = build_zip(
            &[e(MANIFEST_ENTRY, &manifest), e("classes.dex", &dex)],
            CompressionMethod::Deflated,
        );
        let report = inspect(&bytes).expect("inspect");
        assert_eq!(report.manifest.package, "com.example.app");
        assert!(report.all_dex_valid());
    }

    #[test]
    fn missing_manifest_is_an_error() {
        let dex = sample_dex();
        let bytes = stored(&[e("classes.dex", &dex)]);
        assert_eq!(
            inspect(&bytes),
            Err(ApkError::MissingEntry {
                name: MANIFEST_ENTRY
            })
        );
    }

    #[test]
    fn missing_dex_is_a_warning_not_an_error() {
        let manifest = minimal_manifest_axml("com.example.native");
        let bytes = stored(&[e(MANIFEST_ENTRY, &manifest)]);
        let report = inspect(&bytes).expect("inspect");
        assert!(report.dex_files.is_empty());
        assert_eq!(report.warnings, vec![ApkWarning::NoDexFiles]);
        assert!(report.all_dex_valid());
    }

    #[test]
    fn corrupt_zip_data_is_rejected() {
        let good = minimal_apk();
        for bytes in [
            b"this is not a zip file".to_vec(),
            Vec::new(),
            good[..good.len() / 2].to_vec(),
            good[..good.len() - 5].to_vec(),
        ] {
            assert!(
                matches!(inspect(&bytes), Err(ApkError::InvalidArchive { .. })),
                "input of {} bytes",
                bytes.len()
            );
        }
    }

    #[test]
    fn invalid_manifest_data_is_rejected() {
        let dex = sample_dex();
        let text = stored(&[
            e(MANIFEST_ENTRY, b"<manifest package=x/>"),
            e("classes.dex", &dex),
        ]);
        assert!(matches!(
            inspect(&text),
            Err(ApkError::Manifest(crate::apk::ManifestError::Xml(
                crate::apk::axml::AxmlError::NotBinaryXml
            )))
        ));

        let no_package = build_axml(&[start("manifest", vec![]), end()], false);
        let bytes = stored(&[e(MANIFEST_ENTRY, &no_package)]);
        assert_eq!(
            inspect(&bytes),
            Err(ApkError::Manifest(
                crate::apk::ManifestError::MissingPackageName
            ))
        );
    }

    #[test]
    fn multiple_dex_files_are_ordered_numerically() {
        let manifest = minimal_manifest_axml("com.example.multi");
        let dex = sample_dex();
        let bytes = stored(&[
            e(MANIFEST_ENTRY, &manifest),
            e("classes10.dex", &dex),
            e("classes2.dex", &dex),
            e("classes.dex", &dex),
            e("classes1.dex", &dex),
            e("classes02.dex", &dex),
            e("assets/classes3.dex", &dex),
            e("classes.dex.bak", &dex),
        ]);
        let report = inspect(&bytes).expect("inspect");
        let names: Vec<&str> = report
            .dex_files
            .iter()
            .map(|d| d.entry_name.as_str())
            .collect();
        assert_eq!(names, vec!["classes.dex", "classes2.dex", "classes10.dex"]);
        assert!(report.all_dex_valid());
        assert_eq!(report.entry_count, 8);
    }

    #[test]
    fn dex_index_rules() {
        assert_eq!(dex_index("classes.dex"), Some(1));
        assert_eq!(dex_index("classes2.dex"), Some(2));
        assert_eq!(dex_index("classes37.dex"), Some(37));
        assert_eq!(dex_index("classes1.dex"), None);
        assert_eq!(dex_index("classes0.dex"), None);
        assert_eq!(dex_index("classes02.dex"), None);
        assert_eq!(dex_index("classesx.dex"), None);
        assert_eq!(dex_index("classes99999999999.dex"), None);
        assert_eq!(dex_index("sub/classes.dex"), None);
        assert_eq!(dex_index("classes.dex.bak"), None);
    }

    #[test]
    fn malformed_dex_is_reported_against_its_entry() {
        let manifest = minimal_manifest_axml("com.example.baddex");
        let good = sample_dex();
        let garbage = vec![b'x'; 200];
        let bytes = stored(&[
            e(MANIFEST_ENTRY, &manifest),
            e("classes.dex", &good),
            e("classes2.dex", &garbage),
        ]);
        let report = inspect(&bytes).expect("inspection still succeeds");
        assert!(!report.all_dex_valid());
        let failures: Vec<&DexEntryReport> = report.dex_failures().collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].entry_name, "classes2.dex");
        assert_eq!(
            failures[0].outcome,
            Err(DexEntryError::Invalid(DexError::BadMagic))
        );
        assert!(report.dex_files[0].outcome.is_ok());
    }

    #[test]
    fn oversized_manifest_is_rejected() {
        let limits = ApkLimits {
            max_manifest_size: 8,
            ..ApkLimits::default()
        };
        match inspect_bytes(&minimal_apk(), &limits) {
            Err(ApkError::EntryTooLarge { name, max, .. }) => {
                assert_eq!(name, MANIFEST_ENTRY);
                assert_eq!(max, 8);
            }
            other => panic!("unexpected result {other:?}"),
        }
    }

    #[test]
    fn oversized_dex_is_recorded_per_entry() {
        let limits = ApkLimits {
            max_dex_size: 16,
            ..ApkLimits::default()
        };
        let report = inspect_bytes(&minimal_apk(), &limits).expect("inspect");
        assert!(matches!(
            report.dex_files[0].outcome,
            Err(DexEntryError::Unreadable(ApkError::EntryTooLarge {
                max: 16,
                ..
            }))
        ));
    }

    #[test]
    fn compression_bombs_are_not_inflated_past_the_limit() {
        let manifest = minimal_manifest_axml("com.example.bomb");
        let zeros = vec![0u8; 4 * 1024 * 1024];
        let bytes = build_zip(
            &[e(MANIFEST_ENTRY, &manifest), e("classes.dex", &zeros)],
            CompressionMethod::Deflated,
        );
        assert!(
            bytes.len() < 100_000,
            "fixture should be small when compressed"
        );

        let limits = ApkLimits {
            max_dex_size: 1024 * 1024,
            ..ApkLimits::default()
        };
        let report = inspect_bytes(&bytes, &limits).expect("inspect");
        assert!(matches!(
            report.dex_files[0].outcome,
            Err(DexEntryError::Unreadable(ApkError::EntryTooLarge { .. }))
        ));
    }

    #[test]
    fn total_read_budget_is_enforced() {
        let manifest = minimal_manifest_axml("com.example.app");
        let limits = ApkLimits {
            max_total_read: manifest.len() as u64 + 10,
            ..ApkLimits::default()
        };
        assert_eq!(
            inspect_bytes(&minimal_apk(), &limits),
            Err(ApkError::TotalSizeExceeded {
                max: manifest.len() as u64 + 10
            })
        );
    }

    #[test]
    fn too_many_entries_and_oversized_apks_are_rejected() {
        let manifest = minimal_manifest_axml("com.example.many");
        let bytes = stored(&[
            e(MANIFEST_ENTRY, &manifest),
            e("a.txt", b"a"),
            e("b.txt", b"b"),
        ]);
        let few = ApkLimits {
            max_entries: 2,
            ..ApkLimits::default()
        };
        assert_eq!(
            inspect_bytes(&bytes, &few),
            Err(ApkError::TooManyEntries { count: 3, max: 2 })
        );

        let small = ApkLimits {
            max_apk_size: 10,
            ..ApkLimits::default()
        };
        assert_eq!(
            inspect_bytes(&bytes, &small),
            Err(ApkError::FileTooLarge {
                size: bytes.len() as u64,
                max: 10
            })
        );
    }

    #[test]
    fn unusual_but_safe_entry_names_are_accepted() {
        let manifest = minimal_manifest_axml("com.example.names");
        let bytes = stored(&[
            e(MANIFEST_ENTRY, &manifest),
            e("assets/日本語.txt", b"x"),
            e("assets/with space.txt", b"x"),
            e("res/", b""),
        ]);
        let report = inspect(&bytes).expect("inspect");
        assert_eq!(report.entry_count, 4);
        assert_eq!(report.manifest.package, "com.example.names");
    }

    #[test]
    fn unsafe_entry_names_are_rejected() {
        let manifest = minimal_manifest_axml("com.example.unsafe");
        // (placeholder written into the zip, real name patched in, expected reason).
        // Names are patched into a valid archive so the test does not depend on
        // what the ZIP writer is willing to emit.
        let cases: Vec<(&[u8], Vec<u8>, UnsafeEntryReason)> = vec![
            (
                b"aa/evil.txt",
                b"../evil.txt".to_vec(),
                UnsafeEntryReason::ParentTraversal,
            ),
            (
                b"a/xx/b.txt",
                b"a/../b.txt".to_vec(),
                UnsafeEntryReason::ParentTraversal,
            ),
            (
                b"aevil.txt",
                b"/evil.txt".to_vec(),
                UnsafeEntryReason::Absolute,
            ),
            (
                b"Cx/y.txt",
                b"C:/y.txt".to_vec(),
                UnsafeEntryReason::DriveLetter,
            ),
            (
                b"a-b.txt",
                vec![b'a', 0x5c, b'b', b'.', b't', b'x', b't'],
                UnsafeEntryReason::Backslash,
            ),
            (
                b"a_b.txt",
                vec![b'a', 0, b'b', b'.', b't', b'x', b't'],
                UnsafeEntryReason::NulByte,
            ),
        ];
        for (placeholder, actual, expected) in cases {
            let name = String::from_utf8(placeholder.to_vec()).unwrap();
            let mut bytes = stored(&[e(MANIFEST_ENTRY, &manifest), e(&name, b"data")]);
            patch_name(&mut bytes, placeholder, &actual);
            match inspect(&bytes) {
                Err(ApkError::UnsafeEntryName { reason, .. }) => {
                    assert_eq!(reason, expected, "name {actual:?}");
                }
                other => panic!("name {actual:?}: unexpected result {other:?}"),
            }
        }
    }

    #[test]
    fn overlong_entry_names_are_rejected() {
        let manifest = minimal_manifest_axml("com.example.long");
        let long = "a".repeat(2000);
        let bytes = stored(&[e(MANIFEST_ENTRY, &manifest), e(&long, b"x")]);
        assert!(matches!(
            inspect(&bytes),
            Err(ApkError::UnsafeEntryName {
                reason: UnsafeEntryReason::TooLong,
                ..
            })
        ));
    }

    #[test]
    fn duplicate_entry_names_are_rejected() {
        let manifest = minimal_manifest_axml("com.example.dup");
        let mut bytes = stored(&[
            e(MANIFEST_ENTRY, &manifest),
            e("aaa.txt", b"one"),
            e("bbb.txt", b"two"),
        ]);
        let patched = patch_name(&mut bytes, b"bbb.txt", b"aaa.txt");
        let result = inspect(&bytes);

        assert_eq!(patched, 2, "test must patch both ZIP filename records");

        assert!(
            matches!(
                result,
                Err(ApkError::DuplicateEntry { .. }) | Err(ApkError::InvalidArchive { .. })
            ),
            "duplicate ZIP entry names must be rejected"
        );
    }

    #[test]
    fn native_libraries_are_listed_by_name_only() {
        let manifest = minimal_manifest_axml("com.example.jni");
        let bytes = stored(&[
            e(MANIFEST_ENTRY, &manifest),
            e("lib/x86_64/libbar.so", b"elf"),
            e("lib/arm64-v8a/libfoo.so", b"elf"),
            e("lib/arm64-v8a/libaaa.so", b"elf"),
            e("lib/armeabi-v7a/", b""),
            e("lib/readme.txt", b"x"),
            e("lib/foo.so", b"x"),
            e("lib/arm64-v8a/sub/x.so", b"x"),
        ]);
        let report = inspect(&bytes).expect("inspect");
        let libs: Vec<(&str, &str)> = report
            .native_libraries
            .iter()
            .map(|l| (l.abi.as_str(), l.file_name.as_str()))
            .collect();
        assert_eq!(
            libs,
            vec![
                ("arm64-v8a", "libaaa.so"),
                ("arm64-v8a", "libfoo.so"),
                ("x86_64", "libbar.so"),
            ]
        );
        assert_eq!(report.native_libraries[0].size, 3);
    }

    #[test]
    fn unresolved_manifest_references_survive_inspection() {
        let nodes = vec![
            start(
                "manifest",
                vec![plain(
                    "package",
                    TestValue::Str("com.example.res".to_owned()),
                )],
            ),
            start(
                "application",
                vec![android("label", TestValue::Reference(0x7f03_0001))],
            ),
            end(),
            end(),
        ];
        let manifest = build_axml(&nodes, false);
        let bytes = stored(&[e(MANIFEST_ENTRY, &manifest)]);
        let report = inspect(&bytes).expect("inspect");
        assert_eq!(
            report.manifest.label,
            Some(ManifestValue::Unresolved {
                resource_id: 0x7f03_0001
            })
        );
    }

    #[test]
    fn inspect_file_reads_from_disk_and_reports_io_errors() {
        let path = temp_path("ok");
        std::fs::write(&path, minimal_apk()).expect("write temp apk");
        let report = inspect_file(&path, &ApkLimits::default());
        let tiny = ApkLimits {
            max_apk_size: 4,
            ..ApkLimits::default()
        };
        let too_big = inspect_file(&path, &tiny);
        let _ = std::fs::remove_file(&path);

        assert_eq!(report.expect("inspect").manifest.package, "com.example.app");
        assert!(matches!(
            too_big,
            Err(ApkError::FileTooLarge { max: 4, .. })
        ));

        let missing = temp_path("missing");
        let _ = std::fs::remove_file(&missing);
        match inspect_file(&missing, &ApkLimits::default()) {
            Err(ApkError::Io { path, kind }) => {
                assert_eq!(path, Some(missing));
                assert_eq!(kind, io::ErrorKind::NotFound);
            }
            other => panic!("unexpected result {other:?}"),
        }
    }
}
