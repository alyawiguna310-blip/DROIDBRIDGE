//! Bounds-checked little-endian cursor over untrusted bytes.

use super::error::DexError;

#[derive(Debug, Clone)]
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Start reading at `offset`. An offset equal to the length is allowed
    /// (an empty reader); anything beyond it is an error.
    pub(crate) fn at(data: &'a [u8], offset: usize) -> Result<Self, DexError> {
        if offset > data.len() {
            return Err(DexError::UnexpectedEof { offset });
        }
        Ok(Self { data, pos: offset })
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// Everything from the current position to the end.
    pub(crate) fn rest(&self) -> &'a [u8] {
        self.data.get(self.pos..).unwrap_or(&[])
    }

    pub(crate) fn bytes(&mut self, n: usize) -> Result<&'a [u8], DexError> {
        let eof = DexError::UnexpectedEof { offset: self.pos };
        let end = self.pos.checked_add(n).ok_or_else(|| eof.clone())?;
        let slice = self.data.get(self.pos..end).ok_or(eof)?;
        self.pos = end;
        Ok(slice)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, DexError> {
        Ok(self.bytes(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, DexError> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, DexError> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read an unsigned LEB128 value that must fit in 32 bits (at most five
    /// bytes, with the fifth carrying at most four payload bits).
    pub(crate) fn uleb128(&mut self) -> Result<u32, DexError> {
        let start = self.pos;
        let mut result: u32 = 0;
        for i in 0u32..5 {
            let byte = self.u8()?;
            let payload = u32::from(byte & 0x7f);
            if i == 4 && payload > 0x0f {
                return Err(DexError::InvalidUleb128 { offset: start });
            }
            result |= payload << (7 * i);
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }
        Err(DexError::InvalidUleb128 { offset: start })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uleb(bytes: &[u8]) -> Result<u32, DexError> {
        Reader::new(bytes).uleb128()
    }

    #[test]
    fn uleb128_decodes_boundary_values() {
        assert_eq!(uleb(&[0x00]), Ok(0));
        assert_eq!(uleb(&[0x7f]), Ok(127));
        assert_eq!(uleb(&[0x80, 0x01]), Ok(128));
        assert_eq!(uleb(&[0xff, 0xff, 0xff, 0xff, 0x0f]), Ok(u32::MAX));
    }

    #[test]
    fn uleb128_rejects_overflow_overlong_and_truncated() {
        assert_eq!(
            uleb(&[0xff, 0xff, 0xff, 0xff, 0x1f]),
            Err(DexError::InvalidUleb128 { offset: 0 })
        );
        assert_eq!(
            uleb(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x00]),
            Err(DexError::InvalidUleb128 { offset: 0 })
        );
        assert_eq!(uleb(&[0x80]), Err(DexError::UnexpectedEof { offset: 1 }));
        assert_eq!(uleb(&[]), Err(DexError::UnexpectedEof { offset: 0 }));
    }

    #[test]
    fn fixed_width_reads_are_little_endian_and_bounded() {
        let mut r = Reader::new(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07]);
        assert_eq!(r.u16(), Ok(0x0201));
        assert_eq!(r.u32(), Ok(0x0605_0403));
        assert_eq!(r.remaining(), 1);
        assert_eq!(r.u16(), Err(DexError::UnexpectedEof { offset: 6 }));
        assert_eq!(r.u8(), Ok(0x07));
        assert_eq!(r.u8(), Err(DexError::UnexpectedEof { offset: 7 }));
    }

    #[test]
    fn at_allows_end_but_not_beyond() {
        let data = [1u8, 2, 3];
        assert!(Reader::at(&data, 3).is_ok());
        assert!(matches!(
            Reader::at(&data, 4),
            Err(DexError::UnexpectedEof { offset: 4 })
        ));
    }

    #[test]
    fn bytes_does_not_overflow_on_huge_requests() {
        let mut r = Reader::new(&[1, 2, 3]);
        assert!(r.bytes(usize::MAX).is_err());
        assert_eq!(r.remaining(), 3);
    }
}
