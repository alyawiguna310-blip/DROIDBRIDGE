//! DEX header, id tables, class data and code items.

use super::error::DexError;
use super::reader::Reader;

/// Size in bytes of the DEX header.
pub const HEADER_SIZE: u32 = 0x70;

/// Sentinel used by DEX for "no index".
pub const NO_INDEX: u32 = 0xffff_ffff;

const ENDIAN_CONSTANT: u32 = 0x1234_5678;
const SUPPORTED_VERSIONS: [[u8; 3]; 5] = [*b"035", *b"036", *b"037", *b"038", *b"039"];

/// Resource limits applied while parsing untrusted DEX data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DexLimits {
    /// Largest accepted DEX file, in bytes.
    pub max_file_size: usize,
}

impl Default for DexLimits {
    fn default() -> Self {
        Self {
            max_file_size: 64 * 1024 * 1024,
        }
    }
}

/// The validated fields of a DEX header that later stages rely on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexHeader {
    /// Three ASCII digits, for example `*b"035"`.
    pub version: [u8; 3],
    pub checksum: u32,
    pub file_size: u32,
    pub string_ids_size: u32,
    pub string_ids_off: u32,
    pub type_ids_size: u32,
    pub type_ids_off: u32,
    pub proto_ids_size: u32,
    pub proto_ids_off: u32,
    pub field_ids_size: u32,
    pub field_ids_off: u32,
    pub method_ids_size: u32,
    pub method_ids_off: u32,
    pub class_defs_size: u32,
    pub class_defs_off: u32,
    pub data_size: u32,
    pub data_off: u32,
}

/// A `proto_id_item`: a method prototype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtoId {
    pub shorty_idx: u32,
    pub return_type_idx: u32,
    /// Offset of the parameter `type_list`, or `0` for no parameters.
    pub parameters_off: u32,
}

/// A `field_id_item`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldId {
    pub class_idx: u16,
    pub type_idx: u16,
    pub name_idx: u32,
}

/// A `method_id_item`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodId {
    pub class_idx: u16,
    pub proto_idx: u16,
    pub name_idx: u32,
}

/// A `class_def_item`. `NO_INDEX` values are represented as `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassDef {
    pub class_idx: u32,
    pub access_flags: u32,
    pub superclass_idx: Option<u32>,
    pub interfaces_off: u32,
    pub source_file_idx: Option<u32>,
    pub annotations_off: u32,
    /// Offset of the `class_data_item`, or `0` if the class has no members.
    pub class_data_off: u32,
    pub static_values_off: u32,
}

/// An `encoded_field` with its absolute field index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedField {
    pub field_idx: u32,
    pub access_flags: u32,
}

/// An `encoded_method` with its absolute method index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedMethod {
    pub method_idx: u32,
    pub access_flags: u32,
    /// Offset of the `code_item`, or `0` for abstract and native methods.
    pub code_off: u32,
}

/// A parsed `class_data_item`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassData {
    pub static_fields: Vec<EncodedField>,
    pub instance_fields: Vec<EncodedField>,
    pub direct_methods: Vec<EncodedMethod>,
    pub virtual_methods: Vec<EncodedMethod>,
}

/// A parsed `code_item`. Try/catch tables are not parsed; callers must check
/// `tries_size` and refuse to execute code that has them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeItem {
    pub registers_size: u16,
    pub ins_size: u16,
    pub outs_size: u16,
    pub tries_size: u16,
    pub debug_info_off: u32,
    /// The instruction stream in 16-bit code units.
    pub insns: Vec<u16>,
}

/// A method resolved to readable names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodSignature {
    pub class_descriptor: String,
    pub name: String,
    pub shorty: String,
    pub return_type: String,
    pub parameter_types: Vec<String>,
}

/// A validated view of a DEX file. Borrows the bytes it was parsed from.
#[derive(Debug, Clone)]
pub struct DexFile<'a> {
    data: &'a [u8],
    header: DexHeader,
    string_offsets: Vec<u32>,
    type_descriptors: Vec<u32>,
    protos: Vec<ProtoId>,
    fields: Vec<FieldId>,
    methods: Vec<MethodId>,
    class_defs: Vec<ClassDef>,
}

impl<'a> DexFile<'a> {
    /// Parse and validate `data`.
    ///
    /// Checks, in order: size limit, magic and version, header size, endian
    /// tag, declared file size, checksum, section bounds, and the validity of
    /// every index stored in the id tables. Strings, class data and code items
    /// are decoded lazily by the accessors, which are bounds-checked as well.
    pub fn parse(data: &'a [u8], limits: &DexLimits) -> Result<Self, DexError> {
        if data.len() > limits.max_file_size {
            return Err(DexError::FileTooLarge {
                size: data.len(),
                max: limits.max_file_size,
            });
        }
        if data.len() < HEADER_SIZE as usize {
            return Err(DexError::UnexpectedEof { offset: data.len() });
        }

        let mut r = Reader::new(data);
        let magic = r.bytes(8)?;
        if &magic[0..4] != b"dex\n"
            || magic[7] != 0
            || !magic[4..7].iter().all(|b| b.is_ascii_digit())
        {
            return Err(DexError::BadMagic);
        }
        let version = [magic[4], magic[5], magic[6]];
        if !SUPPORTED_VERSIONS.contains(&version) {
            return Err(DexError::UnsupportedVersion(version));
        }

        let checksum = r.u32()?;
        r.bytes(20)?; // SHA-1 signature: present but not verified.
        let file_size = r.u32()?;
        let header_size = r.u32()?;
        if header_size != HEADER_SIZE {
            return Err(DexError::BadHeaderSize(header_size));
        }
        let endian_tag = r.u32()?;
        if endian_tag != ENDIAN_CONSTANT {
            return Err(DexError::UnsupportedEndianTag(endian_tag));
        }
        if usize::try_from(file_size).ok() != Some(data.len()) {
            return Err(DexError::FileSizeMismatch {
                declared: file_size,
                actual: data.len(),
            });
        }

        let computed = adler32(&data[12..]);
        if computed != checksum {
            return Err(DexError::ChecksumMismatch {
                declared: checksum,
                computed,
            });
        }

        let link_size = r.u32()?;
        let _link_off = r.u32()?;
        let _map_off = r.u32()?;
        if link_size != 0 {
            return Err(DexError::Unsupported("statically linked dex (link section)"));
        }

        let header = DexHeader {
            version,
            checksum,
            file_size,
            string_ids_size: r.u32()?,
            string_ids_off: r.u32()?,
            type_ids_size: r.u32()?,
            type_ids_off: r.u32()?,
            proto_ids_size: r.u32()?,
            proto_ids_off: r.u32()?,
            field_ids_size: r.u32()?,
            field_ids_off: r.u32()?,
            method_ids_size: r.u32()?,
            method_ids_off: r.u32()?,
            class_defs_size: r.u32()?,
            class_defs_off: r.u32()?,
            data_size: r.u32()?,
            data_off: r.u32()?,
        };

        let len = data.len();
        check_section(len, "string_ids", header.string_ids_off, header.string_ids_size, 4)?;
        check_section(len, "type_ids", header.type_ids_off, header.type_ids_size, 4)?;
        check_section(len, "proto_ids", header.proto_ids_off, header.proto_ids_size, 12)?;
        check_section(len, "field_ids", header.field_ids_off, header.field_ids_size, 8)?;
        check_section(len, "method_ids", header.method_ids_off, header.method_ids_size, 8)?;
        check_section(len, "class_defs", header.class_defs_off, header.class_defs_size, 32)?;
        check_section(len, "data", header.data_off, header.data_size, 1)?;

        let string_offsets = read_table(data, header.string_ids_off, header.string_ids_size, |r| {
            r.u32()
        })?;
        let type_descriptors = read_table(data, header.type_ids_off, header.type_ids_size, |r| {
            r.u32()
        })?;
        let protos = read_table(data, header.proto_ids_off, header.proto_ids_size, |r| {
            Ok(ProtoId {
                shorty_idx: r.u32()?,
                return_type_idx: r.u32()?,
                parameters_off: r.u32()?,
            })
        })?;
        let fields = read_table(data, header.field_ids_off, header.field_ids_size, |r| {
            Ok(FieldId {
                class_idx: r.u16()?,
                type_idx: r.u16()?,
                name_idx: r.u32()?,
            })
        })?;
        let methods = read_table(data, header.method_ids_off, header.method_ids_size, |r| {
            Ok(MethodId {
                class_idx: r.u16()?,
                proto_idx: r.u16()?,
                name_idx: r.u32()?,
            })
        })?;
        let class_defs = read_table(data, header.class_defs_off, header.class_defs_size, |r| {
            let class_idx = r.u32()?;
            let access_flags = r.u32()?;
            let superclass = r.u32()?;
            let interfaces_off = r.u32()?;
            let source_file = r.u32()?;
            let annotations_off = r.u32()?;
            let class_data_off = r.u32()?;
            let static_values_off = r.u32()?;
            Ok(ClassDef {
                class_idx,
                access_flags,
                superclass_idx: optional_index(superclass),
                interfaces_off,
                source_file_idx: optional_index(source_file),
                annotations_off,
                class_data_off,
                static_values_off,
            })
        })?;

        let n_strings = string_offsets.len();
        let n_types = type_descriptors.len();
        let n_protos = protos.len();

        for &off in &string_offsets {
            check_data_offset("string_data", off, len)?;
        }
        for &descriptor in &type_descriptors {
            check_index("string", descriptor, n_strings)?;
        }
        for p in &protos {
            check_index("string", p.shorty_idx, n_strings)?;
            check_index("type", p.return_type_idx, n_types)?;
            if p.parameters_off != 0 {
                check_data_offset("type_list", p.parameters_off, len)?;
            }
        }
        for f in &fields {
            check_index("type", u32::from(f.class_idx), n_types)?;
            check_index("type", u32::from(f.type_idx), n_types)?;
            check_index("string", f.name_idx, n_strings)?;
        }
        for m in &methods {
            check_index("type", u32::from(m.class_idx), n_types)?;
            check_index("proto", u32::from(m.proto_idx), n_protos)?;
            check_index("string", m.name_idx, n_strings)?;
        }
        for c in &class_defs {
            check_index("type", c.class_idx, n_types)?;
            if let Some(superclass) = c.superclass_idx {
                check_index("type", superclass, n_types)?;
            }
            if let Some(source) = c.source_file_idx {
                check_index("string", source, n_strings)?;
            }
            if c.class_data_off != 0 {
                check_data_offset("class_data", c.class_data_off, len)?;
            }
        }

        Ok(Self {
            data,
            header,
            string_offsets,
            type_descriptors,
            protos,
            fields,
            methods,
            class_defs,
        })
    }

    pub fn header(&self) -> &DexHeader {
        &self.header
    }

    pub fn string_count(&self) -> usize {
        self.string_offsets.len()
    }

    pub fn type_count(&self) -> usize {
        self.type_descriptors.len()
    }

    pub fn protos(&self) -> &[ProtoId] {
        &self.protos
    }

    pub fn fields(&self) -> &[FieldId] {
        &self.fields
    }

    pub fn methods(&self) -> &[MethodId] {
        &self.methods
    }

    pub fn class_defs(&self) -> &[ClassDef] {
        &self.class_defs
    }

    /// Decode string `idx` from its MUTF-8 data item. Unpaired surrogates are
    /// replaced with U+FFFD.
    pub fn string(&self, idx: u32) -> Result<String, DexError> {
        let off = self
            .string_offsets
            .get(idx as usize)
            .copied()
            .ok_or(DexError::IndexOutOfRange {
                kind: "string",
                index: idx,
                limit: limit_u32(self.string_offsets.len()),
            })?;
        read_string_data(self.data, off)
    }

    /// The descriptor (for example `Ljava/lang/Object;` or `I`) of type `idx`.
    pub fn type_descriptor(&self, idx: u32) -> Result<String, DexError> {
        let string_idx = self
            .type_descriptors
            .get(idx as usize)
            .copied()
            .ok_or(DexError::IndexOutOfRange {
                kind: "type",
                index: idx,
                limit: limit_u32(self.type_descriptors.len()),
            })?;
        self.string(string_idx)
    }

    /// Type indices of a prototype's parameters.
    pub fn proto_parameters(&self, proto: &ProtoId) -> Result<Vec<u32>, DexError> {
        if proto.parameters_off == 0 {
            return Ok(Vec::new());
        }
        let mut r = Reader::at(self.data, proto.parameters_off as usize).map_err(|_| {
            DexError::OffsetOutOfBounds {
                what: "type_list",
                offset: proto.parameters_off,
            }
        })?;
        let size = r.u32()? as usize;
        if size > r.remaining() / 2 {
            return Err(DexError::Malformed("type list is larger than the file"));
        }
        let mut out = Vec::with_capacity(size);
        for _ in 0..size {
            let type_idx = u32::from(r.u16()?);
            check_index("type", type_idx, self.type_descriptors.len())?;
            out.push(type_idx);
        }
        Ok(out)
    }

    /// Resolve method `method_idx` to its class, name and prototype.
    pub fn method_signature(&self, method_idx: u32) -> Result<MethodSignature, DexError> {
        let method = self
            .methods
            .get(method_idx as usize)
            .ok_or(DexError::IndexOutOfRange {
                kind: "method",
                index: method_idx,
                limit: limit_u32(self.methods.len()),
            })?;
        let proto = self
            .protos
            .get(usize::from(method.proto_idx))
            .ok_or(DexError::IndexOutOfRange {
                kind: "proto",
                index: u32::from(method.proto_idx),
                limit: limit_u32(self.protos.len()),
            })?;
        let parameter_types = self
            .proto_parameters(proto)?
            .into_iter()
            .map(|t| self.type_descriptor(t))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(MethodSignature {
            class_descriptor: self.type_descriptor(u32::from(method.class_idx))?,
            name: self.string(method.name_idx)?,
            shorty: self.string(proto.shorty_idx)?,
            return_type: self.type_descriptor(proto.return_type_idx)?,
            parameter_types,
        })
    }

    /// Parse a class's `class_data_item`, or `None` if it has no members.
    pub fn class_data(&self, class: &ClassDef) -> Result<Option<ClassData>, DexError> {
        if class.class_data_off == 0 {
            return Ok(None);
        }
        read_class_data(
            self.data,
            class.class_data_off as usize,
            self.fields.len(),
            self.methods.len(),
        )
        .map(Some)
    }

    /// Parse the `code_item` at `code_off` (taken from an [`EncodedMethod`];
    /// `0` means the method has no code and must not be passed here).
    pub fn code_item(&self, code_off: u32) -> Result<CodeItem, DexError> {
        let mut r = Reader::at(self.data, code_off as usize).map_err(|_| {
            DexError::OffsetOutOfBounds {
                what: "code_item",
                offset: code_off,
            }
        })?;
        let registers_size = r.u16()?;
        let ins_size = r.u16()?;
        let outs_size = r.u16()?;
        let tries_size = r.u16()?;
        let debug_info_off = r.u32()?;
        let insns_size = r.u32()?;
        if ins_size > registers_size {
            return Err(DexError::Malformed("ins_size exceeds registers_size"));
        }
        let byte_len = (insns_size as usize)
            .checked_mul(2)
            .ok_or(DexError::Malformed("instruction count overflows"))?;
        let bytes = r.bytes(byte_len)?;
        let insns = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(CodeItem {
            registers_size,
            ins_size,
            outs_size,
            tries_size,
            debug_info_off,
            insns,
        })
    }
}

/// Adler-32 as used by the DEX header checksum (zlib's algorithm).
pub(crate) fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    const CHUNK: usize = 5552;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in data.chunks(CHUNK) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

fn optional_index(value: u32) -> Option<u32> {
    (value != NO_INDEX).then_some(value)
}

fn limit_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn check_index(kind: &'static str, index: u32, limit: usize) -> Result<(), DexError> {
    if (index as usize) < limit {
        Ok(())
    } else {
        Err(DexError::IndexOutOfRange {
            kind,
            index,
            limit: limit_u32(limit),
        })
    }
}

fn check_data_offset(what: &'static str, offset: u32, file_len: usize) -> Result<(), DexError> {
    if (offset as usize) < file_len {
        Ok(())
    } else {
        Err(DexError::OffsetOutOfBounds { what, offset })
    }
}

fn check_section(
    file_len: usize,
    section: &'static str,
    offset: u32,
    count: u32,
    item_size: usize,
) -> Result<(), DexError> {
    if count == 0 {
        return Ok(());
    }
    let err = DexError::SectionOutOfBounds {
        section,
        offset,
        count,
    };
    if offset < HEADER_SIZE {
        return Err(err);
    }
    let bytes = (count as usize)
        .checked_mul(item_size)
        .ok_or_else(|| err.clone())?;
    let end = (offset as usize)
        .checked_add(bytes)
        .ok_or_else(|| err.clone())?;
    if end > file_len {
        return Err(err);
    }
    Ok(())
}

/// Read `count` fixed-size items. Callers must have run [`check_section`]
/// first, which bounds `count` by the file length.
fn read_table<T>(
    data: &[u8],
    offset: u32,
    count: u32,
    mut read: impl FnMut(&mut Reader<'_>) -> Result<T, DexError>,
) -> Result<Vec<T>, DexError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut r = Reader::at(data, offset as usize)?;
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        out.push(read(&mut r)?);
    }
    Ok(out)
}

fn read_string_data(data: &[u8], offset: u32) -> Result<String, DexError> {
    let invalid = DexError::InvalidStringData { offset };
    let mut r = Reader::at(data, offset as usize).map_err(|_| invalid.clone())?;
    let declared_units = r.uleb128().map_err(|_| invalid.clone())?;
    let rest = r.rest();
    let end = rest
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| invalid.clone())?;
    let (text, units) = decode_mutf8(&rest[..end]).ok_or_else(|| invalid.clone())?;
    if units != declared_units as usize {
        return Err(invalid);
    }
    Ok(text)
}

/// Decode Modified UTF-8. Returns the text and the number of UTF-16 code
/// units it contained, or `None` if the bytes are not valid MUTF-8.
fn decode_mutf8(bytes: &[u8]) -> Option<(String, usize)> {
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            0x01..=0x7f => {
                units.push(u16::from(b));
                i += 1;
            }
            0xc0..=0xdf => {
                let b2 = *bytes.get(i + 1)?;
                if b2 & 0xc0 != 0x80 {
                    return None;
                }
                units.push((u16::from(b & 0x1f) << 6) | u16::from(b2 & 0x3f));
                i += 2;
            }
            0xe0..=0xef => {
                let b2 = *bytes.get(i + 1)?;
                let b3 = *bytes.get(i + 2)?;
                if b2 & 0xc0 != 0x80 || b3 & 0xc0 != 0x80 {
                    return None;
                }
                units.push(
                    (u16::from(b & 0x0f) << 12)
                        | (u16::from(b2 & 0x3f) << 6)
                        | u16::from(b3 & 0x3f),
                );
                i += 3;
            }
            _ => return None,
        }
    }
    let count = units.len();
    Some((String::from_utf16_lossy(&units), count))
}

fn read_class_data(
    data: &[u8],
    offset: usize,
    field_count: usize,
    method_count: usize,
) -> Result<ClassData, DexError> {
    let mut r = Reader::at(data, offset)?;
    let static_fields = r.uleb128()?;
    let instance_fields = r.uleb128()?;
    let direct_methods = r.uleb128()?;
    let virtual_methods = r.uleb128()?;
    Ok(ClassData {
        static_fields: read_encoded_fields(&mut r, static_fields, field_count)?,
        instance_fields: read_encoded_fields(&mut r, instance_fields, field_count)?,
        direct_methods: read_encoded_methods(&mut r, direct_methods, method_count)?,
        virtual_methods: read_encoded_methods(&mut r, virtual_methods, method_count)?,
    })
}

/// Every encoded member takes at least two bytes, so a count larger than half
/// the remaining bytes cannot be genuine and is rejected before allocating.
fn checked_member_count(r: &Reader<'_>, count: u32) -> Result<usize, DexError> {
    let count = count as usize;
    if count > r.remaining() / 2 {
        return Err(DexError::Malformed(
            "class data member count exceeds the remaining bytes",
        ));
    }
    Ok(count)
}

fn read_encoded_fields(
    r: &mut Reader<'_>,
    count: u32,
    limit: usize,
) -> Result<Vec<EncodedField>, DexError> {
    let count = checked_member_count(r, count)?;
    let mut out = Vec::with_capacity(count);
    let mut idx: u32 = 0;
    for _ in 0..count {
        let diff = r.uleb128()?;
        idx = idx
            .checked_add(diff)
            .ok_or(DexError::Malformed("field index overflows"))?;
        check_index("field", idx, limit)?;
        let access_flags = r.uleb128()?;
        out.push(EncodedField {
            field_idx: idx,
            access_flags,
        });
    }
    Ok(out)
}

fn read_encoded_methods(
    r: &mut Reader<'_>,
    count: u32,
    limit: usize,
) -> Result<Vec<EncodedMethod>, DexError> {
    let count = checked_member_count(r, count)?;
    let mut out = Vec::with_capacity(count);
    let mut idx: u32 = 0;
    for _ in 0..count {
        let diff = r.uleb128()?;
        idx = idx
            .checked_add(diff)
            .ok_or(DexError::Malformed("method index overflows"))?;
        check_index("method", idx, limit)?;
        let access_flags = r.uleb128()?;
        let code_off = r.uleb128()?;
        out.push(EncodedMethod {
            method_idx: idx,
            access_flags,
            code_off,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dex::testutil::{TestMethod, build_dex, fix_checksum, put_u32};

    fn sample() -> Vec<u8> {
        build_dex(&[
            TestMethod {
                name: "answer",
                return_type: "I",
                params: vec![],
                registers: 1,
                ins: 0,
                outs: 0,
                // const/16 v0, #42 ; return v0
                insns: vec![0x0013, 0x002a, 0x000f],
            },
            TestMethod {
                name: "add",
                return_type: "I",
                params: vec!["I", "I"],
                registers: 3,
                ins: 2,
                outs: 0,
                // add-int v0, v1, v2 ; return v0
                insns: vec![0x0090, 0x0201, 0x000f],
            },
        ])
    }

    fn rd32(bytes: &[u8], off: usize) -> u32 {
        u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
    }

    fn parse(bytes: &[u8]) -> Result<DexFile<'_>, DexError> {
        DexFile::parse(bytes, &DexLimits::default())
    }

    #[test]
    fn adler32_matches_known_vectors() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn parses_a_synthetic_dex() {
        let bytes = sample();
        let dex = parse(&bytes).expect("parse");

        assert_eq!(dex.header().version, *b"035");
        assert_eq!(dex.class_defs().len(), 1);
        assert_eq!(dex.methods().len(), 2);

        let class = dex.class_defs()[0];
        assert_eq!(dex.type_descriptor(class.class_idx).unwrap(), "LTest;");
        assert_eq!(
            class
                .superclass_idx
                .map(|i| dex.type_descriptor(i).unwrap()),
            Some("Ljava/lang/Object;".to_owned())
        );
        assert_eq!(class.source_file_idx, None);

        let data = dex.class_data(&class).unwrap().expect("class data");
        assert!(data.static_fields.is_empty());
        assert!(data.instance_fields.is_empty());
        assert!(data.virtual_methods.is_empty());
        assert_eq!(data.direct_methods.len(), 2);

        let first = data.direct_methods[0];
        let sig = dex.method_signature(first.method_idx).unwrap();
        assert_eq!(sig.class_descriptor, "LTest;");
        assert_eq!(sig.name, "answer");
        assert_eq!(sig.shorty, "I");
        assert_eq!(sig.return_type, "I");
        assert!(sig.parameter_types.is_empty());
        let code = dex.code_item(first.code_off).unwrap();
        assert_eq!(code.registers_size, 1);
        assert_eq!(code.ins_size, 0);
        assert_eq!(code.tries_size, 0);
        assert_eq!(code.insns, vec![0x0013, 0x002a, 0x000f]);

        let second = data.direct_methods[1];
        let sig = dex.method_signature(second.method_idx).unwrap();
        assert_eq!(sig.name, "add");
        assert_eq!(sig.shorty, "III");
        assert_eq!(sig.parameter_types, vec!["I", "I"]);
        let code = dex.code_item(second.code_off).unwrap();
        assert_eq!(code.ins_size, 2);
        assert_eq!(code.insns, vec![0x0090, 0x0201, 0x000f]);
    }

    #[test]
    fn rejects_bad_magic_and_unsupported_version() {
        let mut bytes = sample();
        bytes[0] = b'x';
        assert_eq!(parse(&bytes).unwrap_err(), DexError::BadMagic);

        let mut bytes = sample();
        bytes[7] = 1;
        assert_eq!(parse(&bytes).unwrap_err(), DexError::BadMagic);

        let mut bytes = sample();
        bytes[4..7].copy_from_slice(b"041");
        assert_eq!(
            parse(&bytes).unwrap_err(),
            DexError::UnsupportedVersion(*b"041")
        );
    }

    #[test]
    fn rejects_bad_header_size_and_endian_tag() {
        let mut bytes = sample();
        put_u32(&mut bytes, 0x24, 0x71);
        assert_eq!(parse(&bytes).unwrap_err(), DexError::BadHeaderSize(0x71));

        let mut bytes = sample();
        put_u32(&mut bytes, 0x28, 0x7856_3412);
        assert_eq!(
            parse(&bytes).unwrap_err(),
            DexError::UnsupportedEndianTag(0x7856_3412)
        );
    }

    #[test]
    fn rejects_truncated_input() {
        let bytes = sample();
        assert_eq!(
            parse(&[]).unwrap_err(),
            DexError::UnexpectedEof { offset: 0 }
        );
        assert_eq!(
            parse(&bytes[..0x6f]).unwrap_err(),
            DexError::UnexpectedEof { offset: 0x6f }
        );
        assert_eq!(
            parse(&bytes[..bytes.len() - 1]).unwrap_err(),
            DexError::FileSizeMismatch {
                declared: bytes.len() as u32,
                actual: bytes.len() - 1,
            }
        );
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let mut bytes = sample();
        *bytes.last_mut().unwrap() ^= 0xff;
        assert!(matches!(
            parse(&bytes).unwrap_err(),
            DexError::ChecksumMismatch { .. }
        ));
    }

    #[test]
    fn enforces_the_file_size_limit() {
        let bytes = sample();
        let limits = DexLimits { max_file_size: 16 };
        assert_eq!(
            DexFile::parse(&bytes, &limits).unwrap_err(),
            DexError::FileTooLarge {
                size: bytes.len(),
                max: 16
            }
        );
    }

    #[test]
    fn rejects_link_section() {
        let mut bytes = sample();
        put_u32(&mut bytes, 0x2c, 1);
        fix_checksum(&mut bytes);
        assert!(matches!(
            parse(&bytes).unwrap_err(),
            DexError::Unsupported(_)
        ));
    }

    #[test]
    fn rejects_sections_that_do_not_fit() {
        let mut bytes = sample();
        put_u32(&mut bytes, 0x38, 0x7fff_ffff);
        fix_checksum(&mut bytes);
        assert!(matches!(
            parse(&bytes).unwrap_err(),
            DexError::SectionOutOfBounds {
                section: "string_ids",
                ..
            }
        ));

        let mut bytes = sample();
        put_u32(&mut bytes, 0x3c, 0x10); // inside the header
        fix_checksum(&mut bytes);
        assert!(matches!(
            parse(&bytes).unwrap_err(),
            DexError::SectionOutOfBounds {
                section: "string_ids",
                ..
            }
        ));
    }

    #[test]
    fn rejects_out_of_range_indices() {
        let mut bytes = sample();
        let type_ids_off = rd32(&bytes, 0x44) as usize;
        put_u32(&mut bytes, type_ids_off, 9999);
        fix_checksum(&mut bytes);
        assert!(matches!(
            parse(&bytes).unwrap_err(),
            DexError::IndexOutOfRange {
                kind: "string",
                index: 9999,
                ..
            }
        ));
    }

    #[test]
    fn rejects_string_offsets_outside_the_file() {
        let mut bytes = sample();
        let string_ids_off = rd32(&bytes, 0x3c) as usize;
        put_u32(&mut bytes, string_ids_off, 0x00ff_ffff);
        fix_checksum(&mut bytes);
        assert!(matches!(
            parse(&bytes).unwrap_err(),
            DexError::OffsetOutOfBounds {
                what: "string_data",
                ..
            }
        ));
    }

    #[test]
    fn string_accessor_is_bounds_checked() {
        let bytes = sample();
        let dex = parse(&bytes).unwrap();
        assert!(matches!(
            dex.string(9999),
            Err(DexError::IndexOutOfRange { kind: "string", .. })
        ));
        assert!(matches!(
            dex.type_descriptor(9999),
            Err(DexError::IndexOutOfRange { kind: "type", .. })
        ));
        assert!(matches!(
            dex.method_signature(9999),
            Err(DexError::IndexOutOfRange { kind: "method", .. })
        ));
    }

    #[test]
    fn code_item_rejects_ins_larger_than_registers() {
        let bytes = build_dex(&[TestMethod {
            name: "bad",
            return_type: "V",
            params: vec![],
            registers: 1,
            ins: 3,
            outs: 0,
            insns: vec![0x000e],
        }]);
        let dex = parse(&bytes).unwrap();
        let class = dex.class_defs()[0];
        let method = dex.class_data(&class).unwrap().unwrap().direct_methods[0];
        assert!(matches!(
            dex.code_item(method.code_off),
            Err(DexError::Malformed(_))
        ));
    }

    #[test]
    fn code_item_rejects_instruction_count_beyond_the_file() {
        let mut bytes = sample();
        let code_off = {
            let dex = parse(&bytes).unwrap();
            let class = dex.class_defs()[0];
            dex.class_data(&class).unwrap().unwrap().direct_methods[0].code_off
        };
        put_u32(&mut bytes, code_off as usize + 12, 0x7fff_ffff);
        fix_checksum(&mut bytes);
        let dex = parse(&bytes).unwrap();
        assert!(matches!(
            dex.code_item(code_off),
            Err(DexError::UnexpectedEof { .. })
        ));
        assert!(matches!(
            dex.code_item(0x00ff_ffff),
            Err(DexError::OffsetOutOfBounds {
                what: "code_item",
                ..
            })
        ));
    }

    #[test]
    fn class_data_decodes_delta_encoded_indices() {
        // 2 static fields; field 1 (flags 1) then field 1+2=3 (flags 0).
        let bytes = [2, 0, 0, 0, 1, 1, 2, 0];
        let data = read_class_data(&bytes, 0, 4, 0).unwrap();
        assert_eq!(
            data.static_fields,
            vec![
                EncodedField {
                    field_idx: 1,
                    access_flags: 1
                },
                EncodedField {
                    field_idx: 3,
                    access_flags: 0
                },
            ]
        );
    }

    #[test]
    fn class_data_checks_method_index_range() {
        // 1 direct method: idx 0, flags 1, no code.
        let bytes = [0, 0, 1, 0, 0, 1, 0];
        let data = read_class_data(&bytes, 0, 0, 1).unwrap();
        assert_eq!(
            data.direct_methods,
            vec![EncodedMethod {
                method_idx: 0,
                access_flags: 1,
                code_off: 0
            }]
        );
        assert!(matches!(
            read_class_data(&bytes, 0, 0, 0),
            Err(DexError::IndexOutOfRange { kind: "method", .. })
        ));
    }

    #[test]
    fn class_data_rejects_huge_counts_without_allocating() {
        let bytes = [0xff, 0xff, 0xff, 0xff, 0x0f, 0, 0, 0];
        assert!(matches!(
            read_class_data(&bytes, 0, usize::MAX, usize::MAX),
            Err(DexError::Malformed(_))
        ));
    }

    #[test]
    fn class_data_rejects_truncation_and_index_overflow() {
        assert!(matches!(
            read_class_data(&[0, 0, 1, 0], 0, 0, 1),
            Err(DexError::UnexpectedEof { .. })
        ));
        let overflow = [
            2, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0x0f, 0, 0xff, 0xff, 0xff, 0xff, 0x0f, 0,
        ];
        assert!(matches!(
            read_class_data(&overflow, 0, usize::MAX, 0),
            Err(DexError::Malformed(_))
        ));
        assert!(matches!(
            read_class_data(&[0], 5, 0, 0),
            Err(DexError::UnexpectedEof { .. })
        ));
    }

    #[test]
    fn mutf8_decodes_ascii_nul_and_surrogate_pairs() {
        assert_eq!(decode_mutf8(b"hi"), Some(("hi".to_owned(), 2)));
        assert_eq!(decode_mutf8(&[0xc0, 0x80]), Some(("\0".to_owned(), 1)));
        assert_eq!(decode_mutf8(&[0xc3, 0xa9]), Some(("\u{e9}".to_owned(), 1)));
        assert_eq!(
            decode_mutf8(&[0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80]),
            Some(("\u{1F600}".to_owned(), 2))
        );
    }

    #[test]
    fn mutf8_rejects_invalid_sequences() {
        assert_eq!(decode_mutf8(&[0xff]), None);
        assert_eq!(decode_mutf8(&[0x80]), None);
        assert_eq!(decode_mutf8(&[0xc3]), None);
        assert_eq!(decode_mutf8(&[0xc3, 0x41]), None);
        assert_eq!(decode_mutf8(&[0xe0, 0x80]), None);
        assert_eq!(decode_mutf8(&[0xf0, 0x9f, 0x98, 0x80]), None);
    }

    #[test]
    fn string_data_checks_length_terminator_and_encoding() {
        assert_eq!(read_string_data(&[2, b'h', b'i', 0], 0), Ok("hi".to_owned()));
        let invalid = DexError::InvalidStringData { offset: 0 };
        assert_eq!(read_string_data(&[3, b'h', b'i', 0], 0), Err(invalid.clone()));
        assert_eq!(read_string_data(&[2, b'h', b'i'], 0), Err(invalid.clone()));
        assert_eq!(read_string_data(&[1, 0xff, 0], 0), Err(invalid));
        assert_eq!(
            read_string_data(&[0], 9),
            Err(DexError::InvalidStringData { offset: 9 })
        );
    }
}
