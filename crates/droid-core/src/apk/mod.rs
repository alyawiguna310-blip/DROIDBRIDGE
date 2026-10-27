//! APK inspection.
//!
//! Opens an APK as a ZIP archive, reads `AndroidManifest.xml` (Android binary
//! XML), discovers and validates DEX files with [`crate::dex`], and lists
//! native libraries. Inspection never executes, installs or extracts
//! anything: archive entries are read into bounded in-memory buffers only.
//!
//! ```no_run
//! use droid_core::apk::{inspect_file, ApkLimits};
//!
//! let report = inspect_file("app.apk", &ApkLimits::default())?;
//! println!("package: {}", report.manifest.package);
//! for dex in &report.dex_files {
//!     println!("{}: {:?}", dex.entry_name, dex.outcome.is_ok());
//! }
//! # Ok::<(), droid_core::apk::ApkError>(())
//! ```
//!
//! # Policies
//!
//! - A missing or unparsable `AndroidManifest.xml` fails the inspection.
//! - A missing `classes.dex` does not: the report has no DEX files and carries
//!   [`ApkWarning::NoDexFiles`] (apps with `hasCode=false` are legitimate).
//! - A malformed DEX file does not fail the inspection: the failure is recorded
//!   against that entry in [`ApkReport::dex_files`].
//! - Archives with unsafe entry names, duplicate names, too many entries, or
//!   oversized entries are rejected (see [`ApkLimits`]).
//!
//! A successful inspection says nothing about whether the app can run.

pub mod axml;

mod archive;
mod error;
mod inspect;
mod manifest;
#[cfg(test)]
pub(crate) mod testutil;

pub use error::{ApkError, UnsafeEntryReason};
pub use inspect::{
    ApkLimits, ApkReport, ApkWarning, DexEntryError, DexEntryReport, DexSummary, MANIFEST_ENTRY,
    NativeLibrary, inspect_bytes, inspect_file, inspect_reader, read_dex_entry,
};
pub use manifest::{
    Component, ManifestError, ManifestInfo, ManifestValue, SdkVersion, parse_manifest,
};
