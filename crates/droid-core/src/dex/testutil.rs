//! Synthetic DEX construction for tests.
//!
//! Builds a tiny, well-formed DEX with one class (`LTest;`) whose direct
//! methods are supplied by the test. The SHA-1 signature is left zeroed (the
//! parser does not verify it) and ids are not sorted (the parser does not
//! require ordering).

use super::file::{HEADER_SIZE, NO_INDEX, adler32};

pub(crate) const TEST_CLASS: &str = "LTest;";

pub(crate) struct TestMethod {
    pub name: &'static str,
    pub return_type: &'static str,
    pub params: Vec<&'static str>,
    pub registers: u16,
    pub ins: u16,
    pub outs: u16,
    pub insns: Vec<u16>,
}

#[derive(Default)]
struct Tables {
    strings: Vec<String>,
    /// Descriptor string index for each type.
    types: Vec<u32>,
    /// (shorty string index, return type index, parameter type indices)
    protos: Vec<(u32, u32, Vec<u16>)>,
    /// (class type index, proto index, name string index)
    methods: Vec<(u16, u16, u32)>,
}

impl Tables {
    fn string(&mut self, s: &str) -> u32 {
        match self.strings.iter().position(|x| x == s) {
            Some(i) => i as u32,
            None => {
                self.strings.push(s.to_owned());
                (self.strings.len() - 1) as u32
            }
        }
    }

    fn ty(&mut self, descriptor: &str) -> u16 {
        let s = self.string(descriptor);
        match self.types.iter().position(|&x| x == s) {
            Some(i) => i as u16,
            None => {
                self.types.push(s);
                (self.types.len() - 1) as u16
            }
        }
    }

    fn proto(&mut self, ret: &str, params: &[&str]) -> u16 {
        let mut shorty = String::new();
        shorty.push(shorty_char(ret));
        for p in params {
            shorty.push(shorty_char(p));
        }
        let shorty_idx = self.string(&shorty);
        let ret_idx = u32::from(self.ty(ret));
        let param_idx: Vec<u16> = params.iter().map(|p| self.ty(p)).collect();
        let key = (shorty_idx, ret_idx, param_idx);
        match self.protos.iter().position(|x| *x == key) {
            Some(i) => i as u16,
            None => {
                self.protos.push(key);
                (self.protos.len() - 1) as u16
            }
        }
    }
}

fn shorty_char(descriptor: &str) -> char {
    match descriptor.as_bytes()[0] {
        b'[' => 'L',
        c => c as char,
    }
}

pub(crate) fn put_u32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u16(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn push_uleb(buf: &mut Vec<u8>, mut value: u32) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn align4(buf: &mut Vec<u8>) {
    while buf.len() & 3 != 0 {
        buf.push(0);
    }
}

/// Recompute and store the header checksum after a test edits the bytes.
pub(crate) fn fix_checksum(buf: &mut [u8]) {
    let sum = adler32(&buf[12..]);
    put_u32(buf, 8, sum);
}

/// Build a DEX file containing `LTest;` with `methods` as direct static
/// methods.
pub(crate) fn build_dex(methods: &[TestMethod]) -> Vec<u8> {
    let mut t = Tables::default();
    let class_ty = t.ty(TEST_CLASS);
    let super_ty = t.ty("Ljava/lang/Object;");
    for m in methods {
        let proto = t.proto(m.return_type, &m.params);
        let name = t.string(m.name);
        t.methods.push((class_ty, proto, name));
    }

    let header = HEADER_SIZE as usize;
    let n_strings = t.strings.len();
    let n_types = t.types.len();
    let n_protos = t.protos.len();
    let n_methods = t.methods.len();

    let string_ids_off = header;
    let type_ids_off = string_ids_off + 4 * n_strings;
    let proto_ids_off = type_ids_off + 4 * n_types;
    let method_ids_off = proto_ids_off + 12 * n_protos;
    let class_defs_off = method_ids_off + 8 * n_methods;
    let data_off = class_defs_off + 32;

    let mut buf = vec![0u8; data_off];

    let mut string_offs = Vec::new();
    for s in &t.strings {
        assert!(s.is_ascii(), "test strings must be ASCII");
        string_offs.push(buf.len());
        push_uleb(&mut buf, s.len() as u32);
        buf.extend_from_slice(s.as_bytes());
        buf.push(0);
    }

    let mut param_offs = Vec::new();
    for (_, _, params) in &t.protos {
        if params.is_empty() {
            param_offs.push(0usize);
            continue;
        }
        align4(&mut buf);
        param_offs.push(buf.len());
        buf.extend_from_slice(&(params.len() as u32).to_le_bytes());
        for p in params {
            buf.extend_from_slice(&p.to_le_bytes());
        }
    }

    let mut code_offs = Vec::new();
    for m in methods {
        align4(&mut buf);
        code_offs.push(buf.len());
        for v in [m.registers, m.ins, m.outs, 0u16] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        buf.extend_from_slice(&0u32.to_le_bytes()); // debug_info_off
        buf.extend_from_slice(&(m.insns.len() as u32).to_le_bytes());
        for unit in &m.insns {
            buf.extend_from_slice(&unit.to_le_bytes());
        }
    }

    let class_data_off = buf.len();
    push_uleb(&mut buf, 0); // static fields
    push_uleb(&mut buf, 0); // instance fields
    push_uleb(&mut buf, n_methods as u32); // direct methods
    push_uleb(&mut buf, 0); // virtual methods
    for (i, code_off) in code_offs.iter().enumerate() {
        push_uleb(&mut buf, u32::from(i != 0)); // method_idx_diff
        push_uleb(&mut buf, 0x9); // public static
        push_uleb(&mut buf, *code_off as u32);
    }

    for (i, off) in string_offs.iter().enumerate() {
        put_u32(&mut buf, string_ids_off + 4 * i, *off as u32);
    }
    for (i, descriptor) in t.types.iter().enumerate() {
        put_u32(&mut buf, type_ids_off + 4 * i, *descriptor);
    }
    for (i, (shorty, ret, _)) in t.protos.iter().enumerate() {
        let at = proto_ids_off + 12 * i;
        put_u32(&mut buf, at, *shorty);
        put_u32(&mut buf, at + 4, *ret);
        put_u32(&mut buf, at + 8, param_offs[i] as u32);
    }
    for (i, (class, proto, name)) in t.methods.iter().enumerate() {
        let at = method_ids_off + 8 * i;
        put_u16(&mut buf, at, *class);
        put_u16(&mut buf, at + 2, *proto);
        put_u32(&mut buf, at + 4, *name);
    }
    put_u32(&mut buf, class_defs_off, u32::from(class_ty));
    put_u32(&mut buf, class_defs_off + 4, 0x1); // public
    put_u32(&mut buf, class_defs_off + 8, u32::from(super_ty));
    put_u32(&mut buf, class_defs_off + 12, 0); // interfaces_off
    put_u32(&mut buf, class_defs_off + 16, NO_INDEX); // source_file_idx
    put_u32(&mut buf, class_defs_off + 20, 0); // annotations_off
    put_u32(&mut buf, class_defs_off + 24, class_data_off as u32);
    put_u32(&mut buf, class_defs_off + 28, 0); // static_values_off

    let file_len = buf.len();
    buf[0..8].copy_from_slice(b"dex\n035\0");
    put_u32(&mut buf, 0x20, file_len as u32);
    put_u32(&mut buf, 0x24, HEADER_SIZE);
    put_u32(&mut buf, 0x28, 0x1234_5678);
    put_u32(&mut buf, 0x38, n_strings as u32);
    put_u32(&mut buf, 0x3c, string_ids_off as u32);
    put_u32(&mut buf, 0x40, n_types as u32);
    put_u32(&mut buf, 0x44, type_ids_off as u32);
    put_u32(&mut buf, 0x48, n_protos as u32);
    put_u32(&mut buf, 0x4c, proto_ids_off as u32);
    put_u32(&mut buf, 0x50, 0); // field_ids_size
    put_u32(&mut buf, 0x54, 0);
    put_u32(&mut buf, 0x58, n_methods as u32);
    put_u32(&mut buf, 0x5c, method_ids_off as u32);
    put_u32(&mut buf, 0x60, 1); // class_defs_size
    put_u32(&mut buf, 0x64, class_defs_off as u32);
    put_u32(&mut buf, 0x68, (file_len - data_off) as u32);
    put_u32(&mut buf, 0x6c, data_off as u32);
    fix_checksum(&mut buf);
    buf
}
