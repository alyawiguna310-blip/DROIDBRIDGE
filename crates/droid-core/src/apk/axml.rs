//! Minimal reader for Android binary XML (AXML), the format of the
//! `AndroidManifest.xml` inside an APK.
//!
//! Supported: the file header, UTF-8 and UTF-16 string pools, start and end
//! element chunks, and attributes with string, reference, integer, boolean and
//! float values (anything else is surfaced as [`AttrValue::Other`]).
//!
//! Not supported (skipped or ignored): namespace chunks, CDATA, the resource
//! id map, string styles, and unknown chunk types. Android resource tables
//! (`resources.arsc`) are not read, so references stay unresolved. Plain-text
//! XML and Android App Bundle (protobuf) manifests are rejected with
//! [`AxmlError::NotBinaryXml`]; bytes are never reinterpreted as UTF-8 text.
//!
//! All reads are bounds-checked and all sizes are limited by [`AxmlLimits`].

use std::fmt;

/// Namespace URI of `android:` attributes.
pub const ANDROID_NAMESPACE: &str = "http://schemas.android.com/apk/res/android";

const CHUNK_STRING_POOL: u16 = 0x0001;
const CHUNK_XML: u16 = 0x0003;
const CHUNK_START_ELEMENT: u16 = 0x0102;
const CHUNK_END_ELEMENT: u16 = 0x0103;
const NO_ENTRY: u32 = 0xffff_ffff;
const UTF8_FLAG: u32 = 1 << 8;
const TYPE_REFERENCE: u8 = 0x01;
const TYPE_STRING: u8 = 0x03;
const TYPE_FLOAT: u8 = 0x04;
const TYPE_INT_DEC: u8 = 0x10;
const TYPE_INT_HEX: u8 = 0x11;
const TYPE_INT_BOOLEAN: u8 = 0x12;
const POOL_HEADER_MIN: usize = 28;
const ELEMENT_HEADER_MIN: usize = 16;
const ATTRIBUTE_MIN_SIZE: usize = 20;

/// Resource limits for parsing untrusted binary XML.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxmlLimits {
    /// Maximum number of start plus end element events.
    pub max_events: usize,
    /// Maximum element nesting depth.
    pub max_depth: usize,
    /// Maximum number of strings in the string pool.
    pub max_strings: usize,
}

impl Default for AxmlLimits {
    fn default() -> Self {
        Self {
            max_events: 100_000,
            max_depth: 128,
            max_strings: 100_000,
        }
    }
}

/// A typed attribute value.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    /// A string literal.
    String(String),
    /// A reference to a resource id (for example `@string/app_name`).
    Reference(u32),
    /// A decimal or hexadecimal integer, as its raw 32-bit pattern.
    Int(u32),
    Boolean(bool),
    Float(f32),
    /// Any other typed value, left uninterpreted.
    Other {
        data_type: u8,
        data: u32,
    },
}

/// An element attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct XmlAttribute {
    /// Namespace URI, or `None` for attributes without a namespace.
    pub namespace: Option<String>,
    pub name: String,
    pub value: AttrValue,
}

/// A parsed element boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum XmlEvent {
    StartElement {
        name: String,
        attributes: Vec<XmlAttribute>,
    },
    EndElement {
        name: String,
    },
}

/// Why binary XML was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AxmlError {
    /// The data does not start with a binary XML header.
    NotBinaryXml,
    /// A read ran past the end of the data.
    Truncated { offset: usize },
    /// A structural rule was violated.
    Malformed(&'static str),
    /// A string index is outside the string pool.
    StringIndexOutOfRange { index: u32, count: usize },
    /// A string in the pool is not valid text.
    InvalidString { index: u32 },
    /// A resource limit was exceeded.
    LimitExceeded(&'static str),
}

impl fmt::Display for AxmlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBinaryXml => write!(f, "not Android binary XML"),
            Self::Truncated { offset } => {
                write!(f, "binary XML is truncated at offset {offset:#x}")
            }
            Self::Malformed(what) => write!(f, "malformed binary XML: {what}"),
            Self::StringIndexOutOfRange { index, count } => {
                write!(f, "string index {index} out of range (pool has {count})")
            }
            Self::InvalidString { index } => write!(f, "string {index} is not valid text"),
            Self::LimitExceeded(what) => write!(f, "binary XML limit exceeded: {what}"),
        }
    }
}

impl std::error::Error for AxmlError {}

fn slice(data: &[u8], offset: usize, len: usize) -> Result<&[u8], AxmlError> {
    let end = offset
        .checked_add(len)
        .ok_or(AxmlError::Truncated { offset })?;
    data.get(offset..end).ok_or(AxmlError::Truncated { offset })
}

fn u8_at(data: &[u8], offset: usize) -> Result<u8, AxmlError> {
    Ok(slice(data, offset, 1)?[0])
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16, AxmlError> {
    let b = slice(data, offset, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32, AxmlError> {
    let b = slice(data, offset, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn le16(data: &[u8], offset: usize) -> Option<u16> {
    let b = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

/// UTF-8 pool length prefix: one byte, or two if the high bit is set.
fn read_len8(data: &[u8], offset: usize) -> Option<(usize, usize)> {
    let first = *data.get(offset)?;
    if first & 0x80 != 0 {
        let second = *data.get(offset.checked_add(1)?)?;
        Some((
            (usize::from(first & 0x7f) << 8) | usize::from(second),
            offset.checked_add(2)?,
        ))
    } else {
        Some((usize::from(first), offset.checked_add(1)?))
    }
}

/// UTF-16 pool length prefix: one unit, or two if the high bit is set.
fn read_len16(data: &[u8], offset: usize) -> Option<(usize, usize)> {
    let first = le16(data, offset)?;
    if first & 0x8000 != 0 {
        let second = le16(data, offset.checked_add(2)?)?;
        Some((
            (usize::from(first & 0x7fff) << 16) | usize::from(second),
            offset.checked_add(4)?,
        ))
    } else {
        Some((usize::from(first), offset.checked_add(2)?))
    }
}

struct StringPool<'a> {
    utf8: bool,
    offsets: Vec<u32>,
    data: &'a [u8],
}

impl<'a> StringPool<'a> {
    fn parse(chunk: &'a [u8], limits: &AxmlLimits) -> Result<Self, AxmlError> {
        let header_size = usize::from(u16_at(chunk, 2)?);
        if header_size < POOL_HEADER_MIN {
            return Err(AxmlError::Malformed("string pool header is too small"));
        }
        let count = u32_at(chunk, 8)? as usize;
        let flags = u32_at(chunk, 16)?;
        let strings_start = u32_at(chunk, 20)? as usize;
        if count > limits.max_strings {
            return Err(AxmlError::LimitExceeded("string count"));
        }
        let table_len = count
            .checked_mul(4)
            .ok_or(AxmlError::Malformed("string count overflows"))?;
        slice(chunk, header_size, table_len)?;
        let mut offsets = Vec::with_capacity(count);
        for i in 0..count {
            offsets.push(u32_at(chunk, header_size + i * 4)?);
        }
        let data = chunk
            .get(strings_start..)
            .ok_or(AxmlError::Malformed("string data starts outside the chunk"))?;
        Ok(Self {
            utf8: flags & UTF8_FLAG != 0,
            offsets,
            data,
        })
    }

    fn get(&self, index: u32) -> Result<String, AxmlError> {
        let offset =
            self.offsets
                .get(index as usize)
                .copied()
                .ok_or(AxmlError::StringIndexOutOfRange {
                    index,
                    count: self.offsets.len(),
                })? as usize;
        self.decode(offset)
            .ok_or(AxmlError::InvalidString { index })
    }

    fn decode(&self, offset: usize) -> Option<String> {
        if self.utf8 {
            let (_chars, pos) = read_len8(self.data, offset)?;
            let (byte_len, pos) = read_len8(self.data, pos)?;
            let bytes = self.data.get(pos..pos.checked_add(byte_len)?)?;
            std::str::from_utf8(bytes).ok().map(str::to_owned)
        } else {
            let (len, pos) = read_len16(self.data, offset)?;
            let end = pos.checked_add(len.checked_mul(2)?)?;
            let bytes = self.data.get(pos..end)?;
            let units: Vec<u16> = (0..len)
                .map(|i| u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]))
                .collect();
            String::from_utf16(&units).ok()
        }
    }
}

/// Parse binary XML into a flat list of element events.
///
/// Elements must be balanced, have exactly one root, and every string index
/// must be valid. The returned list is bounded by `limits`.
pub fn parse_binary_xml(data: &[u8], limits: &AxmlLimits) -> Result<Vec<XmlEvent>, AxmlError> {
    if u16_at(data, 0).ok() != Some(CHUNK_XML) {
        return Err(AxmlError::NotBinaryXml);
    }
    let header_size = usize::from(u16_at(data, 2)?);
    let total = u32_at(data, 4)? as usize;
    if header_size < 8 || header_size > total {
        return Err(AxmlError::Malformed("invalid xml header size"));
    }
    if total > data.len() {
        return Err(AxmlError::Truncated { offset: data.len() });
    }
    let data = &data[..total];

    let mut pool: Option<StringPool<'_>> = None;
    let mut events: Vec<XmlEvent> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut seen_root = false;
    let mut offset = header_size;

    while offset < total {
        let chunk_type = u16_at(data, offset)?;
        let chunk_header = usize::from(u16_at(data, offset + 2)?);
        let chunk_size = u32_at(data, offset + 4)? as usize;
        if chunk_size < 8 || chunk_header < 8 || chunk_header > chunk_size {
            return Err(AxmlError::Malformed("invalid chunk header"));
        }
        let end = offset
            .checked_add(chunk_size)
            .filter(|&e| e <= total)
            .ok_or(AxmlError::Truncated { offset })?;
        let chunk = &data[offset..end];

        match chunk_type {
            CHUNK_STRING_POOL => {
                if pool.is_some() {
                    return Err(AxmlError::Malformed("duplicate string pool"));
                }
                pool = Some(StringPool::parse(chunk, limits)?);
            }
            CHUNK_START_ELEMENT => {
                let pool = pool
                    .as_ref()
                    .ok_or(AxmlError::Malformed("element before string pool"))?;
                if stack.is_empty() && seen_root {
                    return Err(AxmlError::Malformed("multiple root elements"));
                }
                if stack.len() >= limits.max_depth {
                    return Err(AxmlError::LimitExceeded("element depth"));
                }
                if events.len() >= limits.max_events {
                    return Err(AxmlError::LimitExceeded("event count"));
                }
                let (name, attributes) = parse_start(chunk, pool)?;
                seen_root = true;
                stack.push(name.clone());
                events.push(XmlEvent::StartElement { name, attributes });
            }
            CHUNK_END_ELEMENT => {
                let pool = pool
                    .as_ref()
                    .ok_or(AxmlError::Malformed("element before string pool"))?;
                if events.len() >= limits.max_events {
                    return Err(AxmlError::LimitExceeded("event count"));
                }
                let name = parse_end(chunk, pool)?;
                let open = stack
                    .pop()
                    .ok_or(AxmlError::Malformed("end element without a start"))?;
                if open != name {
                    return Err(AxmlError::Malformed("mismatched end element"));
                }
                events.push(XmlEvent::EndElement { name });
            }
            // Namespaces, CDATA, the resource id map and unknown chunks.
            _ => {}
        }
        offset = end;
    }

    if !stack.is_empty() {
        return Err(AxmlError::Malformed("unclosed element"));
    }
    if !seen_root {
        return Err(AxmlError::Malformed("no root element"));
    }
    Ok(events)
}

fn parse_start(
    chunk: &[u8],
    pool: &StringPool<'_>,
) -> Result<(String, Vec<XmlAttribute>), AxmlError> {
    let header = usize::from(u16_at(chunk, 2)?);
    if header < ELEMENT_HEADER_MIN {
        return Err(AxmlError::Malformed("element header is too small"));
    }
    let name_idx = u32_at(chunk, header + 4)?;
    let attr_start = usize::from(u16_at(chunk, header + 8)?);
    let attr_size = usize::from(u16_at(chunk, header + 10)?);
    let attr_count = usize::from(u16_at(chunk, header + 12)?);
    if attr_count > 0 && attr_size < ATTRIBUTE_MIN_SIZE {
        return Err(AxmlError::Malformed("attribute size is too small"));
    }
    let name = pool.get(name_idx)?;
    let first = header + attr_start;

    let mut attributes = Vec::with_capacity(attr_count);
    for i in 0..attr_count {
        let base = first + i * attr_size;
        let ns_idx = u32_at(chunk, base)?;
        let attr_name_idx = u32_at(chunk, base + 4)?;
        let data_type = u8_at(chunk, base + 15)?;
        let data = u32_at(chunk, base + 16)?;

        let namespace = if ns_idx == NO_ENTRY {
            None
        } else {
            Some(pool.get(ns_idx)?)
        };
        let value = match data_type {
            TYPE_STRING => AttrValue::String(pool.get(data)?),
            TYPE_REFERENCE => AttrValue::Reference(data),
            TYPE_INT_DEC | TYPE_INT_HEX => AttrValue::Int(data),
            TYPE_INT_BOOLEAN => AttrValue::Boolean(data != 0),
            TYPE_FLOAT => AttrValue::Float(f32::from_bits(data)),
            other => AttrValue::Other {
                data_type: other,
                data,
            },
        };
        attributes.push(XmlAttribute {
            namespace,
            name: pool.get(attr_name_idx)?,
            value,
        });
    }
    Ok((name, attributes))
}

fn parse_end(chunk: &[u8], pool: &StringPool<'_>) -> Result<String, AxmlError> {
    let header = usize::from(u16_at(chunk, 2)?);
    if header < ELEMENT_HEADER_MIN {
        return Err(AxmlError::Malformed("element header is too small"));
    }
    pool.get(u32_at(chunk, header + 4)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apk::testutil::{TestNode, TestValue, android, build_axml, end, plain, start};
    use crate::dex::testutil::put_u32;

    fn limits() -> AxmlLimits {
        AxmlLimits::default()
    }

    fn sample_nodes() -> Vec<TestNode> {
        vec![
            start(
                "manifest",
                vec![
                    plain("package", TestValue::Str("com.example".to_owned())),
                    android("versionCode", TestValue::Int(7)),
                ],
            ),
            start(
                "child",
                vec![
                    android("flag", TestValue::Bool(true)),
                    android("ref", TestValue::Reference(0x7f01_0001)),
                ],
            ),
            end(),
            end(),
        ]
    }

    fn find_chunk(bytes: &[u8], chunk_type: u16) -> usize {
        let mut offset = 8;
        loop {
            let found = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
            if found == chunk_type {
                return offset;
            }
            let size = u32::from_le_bytes([
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ]) as usize;
            offset += size;
        }
    }

    #[test]
    fn parses_utf16_and_utf8_pools_identically() {
        let utf16 = parse_binary_xml(&build_axml(&sample_nodes(), false), &limits()).unwrap();
        let utf8 = parse_binary_xml(&build_axml(&sample_nodes(), true), &limits()).unwrap();
        assert_eq!(utf16, utf8);
        assert_eq!(utf16.len(), 4);

        match &utf16[0] {
            XmlEvent::StartElement { name, attributes } => {
                assert_eq!(name, "manifest");
                assert_eq!(
                    attributes[0],
                    XmlAttribute {
                        namespace: None,
                        name: "package".to_owned(),
                        value: AttrValue::String("com.example".to_owned()),
                    }
                );
                assert_eq!(attributes[1].namespace.as_deref(), Some(ANDROID_NAMESPACE));
                assert_eq!(attributes[1].name, "versionCode");
                assert_eq!(attributes[1].value, AttrValue::Int(7));
            }
            other => panic!("unexpected event {other:?}"),
        }
        match &utf16[1] {
            XmlEvent::StartElement { attributes, .. } => {
                assert_eq!(attributes[0].value, AttrValue::Boolean(true));
                assert_eq!(attributes[1].value, AttrValue::Reference(0x7f01_0001));
            }
            other => panic!("unexpected event {other:?}"),
        }
        assert_eq!(
            utf16[3],
            XmlEvent::EndElement {
                name: "manifest".to_owned()
            }
        );
    }

    #[test]
    fn decodes_non_ascii_text_in_both_encodings() {
        let text = "héllo 😀 日本語";
        for utf8 in [false, true] {
            let nodes = vec![
                start(
                    "manifest",
                    vec![plain("package", TestValue::Str(text.to_owned()))],
                ),
                end(),
            ];
            let events = parse_binary_xml(&build_axml(&nodes, utf8), &limits()).unwrap();
            match &events[0] {
                XmlEvent::StartElement { attributes, .. } => {
                    assert_eq!(attributes[0].value, AttrValue::String(text.to_owned()));
                }
                other => panic!("unexpected event {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_input_that_is_not_binary_xml() {
        let inputs: [&[u8]; 4] = [b"", b"<manifest/>", &[0x03], b"PK-not-xml-at-all"];
        for input in inputs {
            assert_eq!(
                parse_binary_xml(input, &limits()),
                Err(AxmlError::NotBinaryXml),
                "input {input:?}"
            );
        }
        assert!(matches!(
            parse_binary_xml(&[0x03, 0x00], &limits()),
            Err(AxmlError::Truncated { .. })
        ));
    }

    #[test]
    fn every_truncation_is_rejected() {
        for utf8 in [false, true] {
            let bytes = build_axml(&sample_nodes(), utf8);
            for cut in 0..bytes.len() {
                assert!(
                    parse_binary_xml(&bytes[..cut], &limits()).is_err(),
                    "cut at {cut} was accepted"
                );
            }
        }
    }

    #[test]
    fn corrupted_bytes_never_panic() {
        for utf8 in [false, true] {
            let original = build_axml(&sample_nodes(), utf8);
            for index in 0..original.len() {
                for flip in [0x01u8, 0x80, 0xff] {
                    let mut mutated = original.clone();
                    mutated[index] ^= flip;
                    let _ = parse_binary_xml(&mutated, &limits());
                }
            }
        }
    }

    #[test]
    fn rejects_string_index_outside_the_pool() {
        let mut bytes = build_axml(&sample_nodes(), false);
        let element = find_chunk(&bytes, CHUNK_START_ELEMENT);
        put_u32(&mut bytes, element + 20, 0xffff);
        assert!(matches!(
            parse_binary_xml(&bytes, &limits()),
            Err(AxmlError::StringIndexOutOfRange { index: 0xffff, .. })
        ));
    }

    #[test]
    fn rejects_zero_sized_chunks() {
        let mut bytes = build_axml(&sample_nodes(), false);
        let element = find_chunk(&bytes, CHUNK_START_ELEMENT);
        put_u32(&mut bytes, element + 4, 0);
        assert_eq!(
            parse_binary_xml(&bytes, &limits()),
            Err(AxmlError::Malformed("invalid chunk header"))
        );
    }

    #[test]
    fn enforces_depth_and_event_limits() {
        let mut nodes = Vec::new();
        for _ in 0..200 {
            nodes.push(start("a", vec![]));
        }
        for _ in 0..200 {
            nodes.push(end());
        }
        assert!(matches!(
            parse_binary_xml(&build_axml(&nodes, false), &limits()),
            Err(AxmlError::LimitExceeded(_))
        ));

        let few = AxmlLimits {
            max_events: 3,
            ..AxmlLimits::default()
        };
        assert!(matches!(
            parse_binary_xml(&build_axml(&sample_nodes(), false), &few),
            Err(AxmlError::LimitExceeded(_))
        ));
    }

    #[test]
    fn rejects_unbalanced_or_rootless_documents() {
        let unclosed = vec![start("a", vec![]), start("b", vec![]), end()];
        assert_eq!(
            parse_binary_xml(&build_axml(&unclosed, false), &limits()),
            Err(AxmlError::Malformed("unclosed element"))
        );
        assert_eq!(
            parse_binary_xml(&build_axml(&[], false), &limits()),
            Err(AxmlError::Malformed("no root element"))
        );
        let two_roots = vec![start("a", vec![]), end(), start("b", vec![]), end()];
        assert_eq!(
            parse_binary_xml(&build_axml(&two_roots, false), &limits()),
            Err(AxmlError::Malformed("multiple root elements"))
        );
    }
}
