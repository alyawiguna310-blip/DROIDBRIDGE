//! DEX file parsing.
//!
//! DEX files come from untrusted APKs, so every read is bounds-checked and
//! every failure is a [`DexError`]; parsing never panics and never allocates
//! based on an unchecked count from the file.
//!
//! # Scope
//!
//! Implemented: header validation (magic, version `035`-`039`, endian tag,
//! file size, Adler-32 checksum, section bounds), the string, type, proto,
//! field, method and class-definition tables with index validation, class data
//! (`class_data_item`), and code items (`code_item`) without try/catch tables.
//!
//! Not implemented: the SHA-1 signature is not verified; the map list, link
//! section, annotations, static values, debug info, try/catch handlers and
//! DEX versions other than `035`-`039` are not interpreted. Files that
//! declare a link section are rejected rather than half-loaded.
//!
//! This module only parses. It does not execute anything.

mod error;
mod file;
mod reader;
#[cfg(test)]
pub(crate) mod testutil;

pub use error::DexError;
pub use file::{
    ClassData, ClassDef, CodeItem, DexFile, DexHeader, DexLimits, EncodedField, EncodedMethod,
    FieldId, HEADER_SIZE, MethodId, MethodSignature, NO_INDEX, ProtoId,
};
