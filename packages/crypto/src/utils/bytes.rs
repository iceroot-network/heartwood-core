//! Little-endian byte cursors for the transaction and block codecs.
//!
//! [`Reader`] never panics: a read past the end returns [`UnexpectedEnd`] and leaves the cursor
//! where it was. [`Writer`] appends to a growable buffer; size limits are the caller's rule.

use crate::errors::UnexpectedEnd;

/// A cursor over untrusted bytes.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    /// A cursor at the start of `bytes`.
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, offset: 0 }
    }

    /// Bytes read so far.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Bytes not read yet.
    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    /// Whether every byte has been read.
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// The bytes not read yet, without moving the cursor.
    pub fn rest(&self) -> &'a [u8] {
        self.bytes.get(self.offset..).unwrap_or_default()
    }

    /// The next `n` bytes.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], UnexpectedEnd> {
        let end = self.offset.checked_add(n).ok_or(self.end(n))?;
        let slice = self.bytes.get(self.offset..end).ok_or(self.end(n))?;
        self.offset = end;
        Ok(slice)
    }

    /// The next `N` bytes as an array.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], UnexpectedEnd> {
        let slice = self.bytes(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    /// The next byte, without moving the cursor.
    pub fn peek_u8(&self) -> Result<u8, UnexpectedEnd> {
        self.bytes.get(self.offset).copied().ok_or(self.end(1))
    }

    /// The next byte.
    pub fn u8(&mut self) -> Result<u8, UnexpectedEnd> {
        let [byte] = self.array::<1>()?;
        Ok(byte)
    }

    /// The next two bytes as a little-endian `u16`.
    pub fn u16_le(&mut self) -> Result<u16, UnexpectedEnd> {
        self.array().map(u16::from_le_bytes)
    }

    /// The next four bytes as a little-endian `u32`.
    pub fn u32_le(&mut self) -> Result<u32, UnexpectedEnd> {
        self.array().map(u32::from_le_bytes)
    }

    /// The next eight bytes as a little-endian `u64`.
    pub fn u64_le(&mut self) -> Result<u64, UnexpectedEnd> {
        self.array().map(u64::from_le_bytes)
    }

    fn end(&self, needed: usize) -> UnexpectedEnd {
        UnexpectedEnd {
            offset: self.offset,
            needed,
            remaining: self.remaining(),
        }
    }
}

/// An append-only byte buffer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    /// An empty buffer.
    pub fn new() -> Writer {
        Writer::default()
    }

    /// An empty buffer with room for `capacity` bytes.
    pub fn with_capacity(capacity: usize) -> Writer {
        Writer {
            bytes: Vec::with_capacity(capacity),
        }
    }

    /// Bytes written so far.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether nothing has been written.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Append `bytes`.
    pub fn bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// Append one byte.
    pub fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    /// Append a little-endian `u16`.
    pub fn u16_le(&mut self, value: u16) {
        self.bytes(&value.to_le_bytes());
    }

    /// Append a little-endian `u32`.
    pub fn u32_le(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    /// Append a little-endian `u64`.
    pub fn u64_le(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    /// The bytes written.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    /// The buffer, consumed.
    pub fn into_vec(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_and_stops_at_the_end() {
        let bytes = [1, 2, 0, 3, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 9];
        let mut r = Reader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u16_le().unwrap(), 2);
        assert_eq!(r.u32_le().unwrap(), 3);
        assert_eq!(r.peek_u8().unwrap(), 4);
        assert_eq!(r.u64_le().unwrap(), 4);
        assert_eq!(r.offset(), 15);
        assert_eq!(r.rest(), &[9]);
        let err = r.u16_le().unwrap_err();
        assert_eq!(
            err,
            UnexpectedEnd {
                offset: 15,
                needed: 2,
                remaining: 1
            }
        );
        assert_eq!(r.offset(), 15, "a failed read does not move the cursor");
        assert_eq!(r.bytes(1).unwrap(), &[9]);
        assert!(r.is_empty());
        assert!(r.peek_u8().is_err());
        assert!(r.bytes(usize::MAX).is_err());
    }

    #[test]
    fn writes_little_endian() {
        let mut w = Writer::with_capacity(15);
        w.u8(1);
        w.u16_le(2);
        w.u32_le(3);
        w.u64_le(4);
        assert_eq!(w.len(), 15);
        assert_eq!(w.into_vec(), [1, 2, 0, 3, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0]);
    }
}
