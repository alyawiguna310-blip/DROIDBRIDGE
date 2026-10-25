//! Synthetic APK fixtures for tests: ZIP archives, Android binary XML, and a
//! tiny DEX. Nothing here is derived from a real application.

use crate::dex::testutil::{TestMethod, build_dex};
use std::io::{Cursor, Write};
use zip::CompressionMethod;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

const ANDROID_NS: &str = "http://schemas.android.com/apk/res/android";
const NO_ENTRY: u32 = 0xffff_ffff;

/// Build a ZIP archive. Names ending in `/` become directory entries.
pub(crate) fn build_zip(entries: &[(&str, &[u8])], method: CompressionMethod) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut cursor);
        for (name, data) in entries {
            let options = SimpleFileOptions::default().compression_method(method);
            if name.ends_with('/') {
                writer.add_directory(*name, options).expect("add directory");
            } else {
                writer.start_file(*name, options).expect("start file");
                writer.write_all(data).expect("write entry");
            }
        }
        writer.finish().expect("finish zip");
    }
    cursor.into_inner()
}

/// Replace every occurrence of `from` with `to` (same length). Used to give a
/// valid archive an entry name the ZIP writer would not produce itself. Both
/// the local header and the central directory entry must be patched.
pub(crate) fn patch_name(bytes: &mut [u8], from: &[u8], to: &[u8]) -> usize {
    assert_eq!(from.len(), to.len(), "replacement must keep the length");
    let mut count = 0;
    let mut i = 0;
    while i + from.len() <= bytes.len() {
        if bytes[i..i + from.len()] == *from {
            bytes[i..i + from.len()].copy_from_slice(to);
            count += 1;
            i += from.len();
        } else {
            i += 1;
        }
    }
    assert!(count >= 2, "name should appear in both zip headers");
    count
}

/// A DEX with one class and one trivial method.
pub(crate) fn sample_dex() -> Vec<u8> {
    build_dex(&[TestMethod {
        name: "answer",
        return_type: "I",
        params: vec![],
        registers: 1,
        ins: 0,
        outs: 0,
        insns: vec![0x0013, 0x002a, 0x000f],
    }])
}

pub(crate) enum TestValue {
    Str(String),
    Reference(u32),
    Int(u32),
    Bool(bool),
}

pub(crate) struct TestAttr {
    ns: Option<&'static str>,
    name: String,
    value: TestValue,
}

pub(crate) enum TestNode {
    Start { name: String, attrs: Vec<TestAttr> },
    End,
}

/// An attribute in the `android:` namespace.
pub(crate) fn android(name: &str, value: TestValue) -> TestAttr {
    TestAttr {
        ns: Some(ANDROID_NS),
        name: name.to_owned(),
        value,
    }
}

/// An attribute without a namespace.
pub(crate) fn plain(name: &str, value: TestValue) -> TestAttr {
    TestAttr {
        ns: None,
        name: name.to_owned(),
        value,
    }
}

pub(crate) fn start(name: &str, attrs: Vec<TestAttr>) -> TestNode {
    TestNode::Start {
        name: name.to_owned(),
        attrs,
    }
}

pub(crate) fn end() -> TestNode {
    TestNode::End
}

#[derive(Default)]
struct Pool {
    strings: Vec<String>,
}

impl Pool {
    fn intern(&mut self, s: &str) -> u32 {
        match self.strings.iter().position(|x| x == s) {
            Some(i) => i as u32,
            None => {
                self.strings.push(s.to_owned());
                (self.strings.len() - 1) as u32
            }
        }
    }
}

fn put_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

/// Serialize `nodes` as Android binary XML, using a UTF-8 or UTF-16 string
/// pool. Every string must be shorter than 128 bytes for the UTF-8 pool.
pub(crate) fn build_axml(nodes: &[TestNode], utf8: bool) -> Vec<u8> {
    let mut pool = Pool::default();
    let mut body = Vec::new();
    let mut open: Vec<u32> = Vec::new();

    for node in nodes {
        match node {
            TestNode::Start { name, attrs } => {
                let name_idx = pool.intern(name);
                open.push(name_idx);
                put_u16(&mut body, 0x0102);
                put_u16(&mut body, 16);
                put_u32(&mut body, (36 + 20 * attrs.len()) as u32);
                put_u32(&mut body, 1); // line number
                put_u32(&mut body, NO_ENTRY); // comment
                put_u32(&mut body, NO_ENTRY); // element namespace
                put_u32(&mut body, name_idx);
                put_u16(&mut body, 20); // attribute start
                put_u16(&mut body, 20); // attribute size
                put_u16(&mut body, attrs.len() as u16);
                put_u16(&mut body, 0); // id index
                put_u16(&mut body, 0); // class index
                put_u16(&mut body, 0); // style index
                for attr in attrs {
                    let ns = match attr.ns {
                        Some(ns) => pool.intern(ns),
                        None => NO_ENTRY,
                    };
                    let attr_name = pool.intern(&attr.name);
                    let (raw, data_type, data) = match &attr.value {
                        TestValue::Str(s) => {
                            let idx = pool.intern(s);
                            (idx, 0x03u8, idx)
                        }
                        TestValue::Reference(id) => (NO_ENTRY, 0x01, *id),
                        TestValue::Int(n) => (NO_ENTRY, 0x10, *n),
                        TestValue::Bool(b) => (NO_ENTRY, 0x12, if *b { 0xffff_ffff } else { 0 }),
                    };
                    put_u32(&mut body, ns);
                    put_u32(&mut body, attr_name);
                    put_u32(&mut body, raw);
                    put_u16(&mut body, 8); // typed value size
                    body.push(0); // reserved
                    body.push(data_type);
                    put_u32(&mut body, data);
                }
            }
            TestNode::End => {
                let name_idx = open.pop().expect("balanced test nodes");
                put_u16(&mut body, 0x0103);
                put_u16(&mut body, 16);
                put_u32(&mut body, 24);
                put_u32(&mut body, 1); // line number
                put_u32(&mut body, NO_ENTRY); // comment
                put_u32(&mut body, NO_ENTRY); // element namespace
                put_u32(&mut body, name_idx);
            }
        }
    }

    let mut blob = Vec::new();
    let mut offsets = Vec::new();
    for s in &pool.strings {
        offsets.push(blob.len() as u32);
        if utf8 {
            assert!(s.len() < 128, "utf-8 pool strings must be short");
            blob.push(s.chars().count() as u8);
            blob.push(s.len() as u8);
            blob.extend_from_slice(s.as_bytes());
            blob.push(0);
        } else {
            let units: Vec<u16> = s.encode_utf16().collect();
            assert!(units.len() < 0x8000, "utf-16 pool strings must be short");
            blob.extend_from_slice(&(units.len() as u16).to_le_bytes());
            for unit in units {
                blob.extend_from_slice(&unit.to_le_bytes());
            }
            blob.extend_from_slice(&[0, 0]);
        }
    }
    while blob.len() & 3 != 0 {
        blob.push(0);
    }

    let count = pool.strings.len();
    let strings_start = 28 + 4 * count;
    let mut pool_chunk = Vec::new();
    put_u16(&mut pool_chunk, 0x0001);
    put_u16(&mut pool_chunk, 28);
    put_u32(&mut pool_chunk, (strings_start + blob.len()) as u32);
    put_u32(&mut pool_chunk, count as u32);
    put_u32(&mut pool_chunk, 0); // style count
    put_u32(&mut pool_chunk, if utf8 { 0x100 } else { 0 });
    put_u32(&mut pool_chunk, strings_start as u32);
    put_u32(&mut pool_chunk, 0); // styles start
    for offset in offsets {
        put_u32(&mut pool_chunk, offset);
    }
    pool_chunk.extend_from_slice(&blob);

    let mut file = Vec::new();
    put_u16(&mut file, 0x0003);
    put_u16(&mut file, 8);
    put_u32(&mut file, (8 + pool_chunk.len() + body.len()) as u32);
    file.extend_from_slice(&pool_chunk);
    file.extend_from_slice(&body);
    file
}

/// A manifest with only a `package` attribute.
pub(crate) fn minimal_manifest_axml(package: &str) -> Vec<u8> {
    build_axml(
        &[
            start(
                "manifest",
                vec![plain("package", TestValue::Str(package.to_owned()))],
            ),
            end(),
        ],
        false,
    )
}

/// A manifest exercising every field the parser reads.
pub(crate) fn sample_manifest_axml(utf8: bool) -> Vec<u8> {
    let s = |text: &str| TestValue::Str(text.to_owned());
    let nodes = vec![
        start(
            "manifest",
            vec![
                plain("package", s("com.example.app")),
                android("versionCode", TestValue::Int(7)),
                android("versionName", s("1.2.3")),
            ],
        ),
        start(
            "uses-sdk",
            vec![
                android("minSdkVersion", TestValue::Int(21)),
                android("targetSdkVersion", TestValue::Int(34)),
            ],
        ),
        end(),
        start(
            "uses-permission",
            vec![android("name", s("android.permission.INTERNET"))],
        ),
        end(),
        start(
            "permission",
            vec![android("name", s("com.example.app.PRIVATE"))],
        ),
        end(),
        start("application", vec![android("label", s("Example"))]),
        start("activity", vec![android("name", s(".MainActivity"))]),
        end(),
        start(
            "service",
            vec![android("name", s("com.example.app.SyncService"))],
        ),
        end(),
        start("receiver", vec![android("name", s("BootReceiver"))]),
        end(),
        end(),
        end(),
    ];
    build_axml(&nodes, utf8)
}
