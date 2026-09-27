//! The protobuf layer: the reader, the writer and the message macro.
//!
//! The reader reproduces the decoder that the reference's protobuf library (protobufjs 7.2.4, its
//! generated static code over a `BufferReader`) runs, malformed bytes included: a known field is
//! read by its declared type whatever its wire type; `uint32` reads at most five bytes and skips
//! five more when the fifth has its high bit set; `string` is cut at the end of the buffer without
//! an error; a nested message ends at its declared length, which is not checked against the
//! buffer; unknown fields are skipped by wire type, groups iteratively at any depth. The writer
//! writes present fields only, in field-number order, and the map in its own-key order.
//!
//! `message!` declares a message once, one line per field:
//!
//! ```text
//! message! {
//!     /// Docs of the message.
//!     GetBlocksRequest {
//!         /// Docs of the field.
//!         1 last_block_height "lastBlockHeight": u32,
//!         5 headers "headers": message(Headers),
//!     }
//! }
//! ```
//!
//! Each line gives the field number, the Rust name, the reference's (JavaScript) name and the
//! kind: `u32`, `bool`, `string`, `bytes`, `message(T)`, `strings` (repeated string),
//! `messages(T)` (repeated message) or `map(T)` (the string-keyed map of messages).

use crate::errors::DecodeError;
use crate::js;
use crate::messages::peer::{Plugin, Plugins};
use crate::messages::{Field, WireOrder};

#[cfg(test)]
pub(crate) mod tests;

/// The wire types the writer uses: varint and length-delimited.
const VARINT: u32 = 0;
const DELIMITED: u32 = 2;

/// A position in a payload, read as protobufjs's `BufferReader` reads. One reader serves a
/// message and every message nested in it, so a field that runs past the end of its nested
/// message moves the position of the enclosing one too, as in the reference.
#[derive(Debug)]
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// A reader at the start of `bytes`.
    pub(crate) fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader { bytes, pos: 0 }
    }

    /// The next byte. Past the end, the reference reads `undefined`, keeps going and fails when
    /// the value is complete; failing at once is the same verdict.
    fn byte(&mut self) -> Result<u8, DecodeError> {
        let byte = *self
            .bytes
            .get(self.pos)
            .ok_or(DecodeError::IndexOutOfRange)?;
        self.pos = self.pos.saturating_add(1);
        Ok(byte)
    }

    /// Move `count` bytes on, refusing to pass the end.
    fn advance(&mut self, count: usize) -> Result<(), DecodeError> {
        self.pos = self
            .pos
            .checked_add(count)
            .filter(|&end| end <= self.bytes.len())
            .ok_or(DecodeError::IndexOutOfRange)?;
        Ok(())
    }

    /// `uint32()`: the low 7 bits of up to four bytes and the low 4 bits of a fifth. When the
    /// fifth has its high bit set, the next five bytes are skipped whatever they hold.
    pub(crate) fn uint32(&mut self) -> Result<u32, DecodeError> {
        let mut value = 0u32;
        for (shift, mask) in [(0, 0x7f), (7, 0x7f), (14, 0x7f), (21, 0x7f), (28, 0x0f)] {
            let byte = self.byte()?;
            value |= u32::from(byte & mask) << shift;
            if byte < 0x80 {
                return Ok(value);
            }
        }
        self.advance(5)?;
        Ok(value)
    }

    /// `bool()`: a `uint32` other than 0.
    fn bool(&mut self) -> Result<bool, DecodeError> {
        Ok(self.uint32()? != 0)
    }

    /// A length: a `uint32`, as a `usize`.
    fn length(&mut self) -> Result<usize, DecodeError> {
        Ok(usize::try_from(self.uint32()?).unwrap_or(usize::MAX))
    }

    /// `bytes()`: a length and that many bytes, which must all be there.
    fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let length = self.length()?;
        let start = self.pos;
        self.advance(length)?;
        self.bytes
            .get(start..self.pos)
            .ok_or(DecodeError::IndexOutOfRange)
    }

    /// `string()`: a length and the lossy UTF-8 text of that many bytes, cut at the end of the
    /// payload without an error.
    fn string(&mut self) -> Result<String, DecodeError> {
        let length = self.length()?;
        let end = self.pos.saturating_add(length).min(self.bytes.len());
        let text = js::utf8_lossy(self.bytes.get(self.pos..end).unwrap_or_default());
        self.pos = end;
        Ok(text)
    }

    /// The end of a nested message whose length comes next: its position plus that length, which
    /// may lie past the end of the payload (the reads then fail).
    fn nested_end(&mut self) -> Result<usize, DecodeError> {
        let length = self.length()?;
        Ok(self.pos.saturating_add(length))
    }

    /// `skipType(wireType)` for an unknown field: a varint (of any length), 8 bytes, a length and
    /// that many bytes, a group up to its end-group tag, or 4 bytes. Wire types 4 (outside a
    /// group), 6 and 7 are refused. The reference recurses into nested groups; this loop counts
    /// them instead, so no depth overflows the stack.
    fn skip(&mut self, wire_type: u32) -> Result<(), DecodeError> {
        let mut open_groups = 0usize;
        let mut wire_type = wire_type;
        loop {
            match wire_type {
                0 => while self.byte()? >= 0x80 {},
                1 => self.advance(8)?,
                2 => {
                    let length = self.length()?;
                    self.advance(length)?;
                }
                3 => open_groups = open_groups.saturating_add(1),
                4 if open_groups > 0 => open_groups = open_groups.saturating_sub(1),
                5 => self.advance(4)?,
                other => return Err(DecodeError::InvalidWireType(other as u8)),
            }
            if open_groups == 0 {
                return Ok(());
            }
            wire_type = self.uint32()? & 7;
        }
    }

    /// Read the fields of a message up to `end`: each tag, then the field `read_field` knows by
    /// its number (`Ok(false)` for an unknown number, which is skipped by its wire type).
    pub(crate) fn fields(
        &mut self,
        end: usize,
        mut read_field: impl FnMut(&mut Reader<'a>, u32) -> Result<bool, DecodeError>,
    ) -> Result<(), DecodeError> {
        while self.pos < end {
            let tag = self.uint32()?;
            if !read_field(self, tag >> 3)? {
                self.skip(tag & 7)?;
            }
        }
        Ok(())
    }
}

/// Write `value` as a varint, as protobufjs's `uint32` does.
fn write_varint(out: &mut Vec<u8>, value: u32) {
    let mut rest = value;
    while rest >= 0x80 {
        out.push(rest as u8 | 0x80);
        rest >>= 7;
    }
    out.push(rest as u8);
}

/// Write the tag of field `number` with `wire_type`.
fn write_tag(out: &mut Vec<u8>, number: u32, wire_type: u32) {
    write_varint(out, number << 3 | wire_type);
}

/// Write field `number` as a length and `bytes`. A length above `u32::MAX` wraps, as the
/// reference's `uint32` does; no payload the port carries is that long.
fn write_delimited(out: &mut Vec<u8>, number: u32, bytes: &[u8]) {
    write_tag(out, number, DELIMITED);
    write_varint(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

/// A protobuf message, read and written by the code the `message!` macro generates.
pub(crate) trait Message: Sized {
    /// Read a message whose fields end at `end`.
    fn read(reader: &mut Reader<'_>, end: usize) -> Result<Self, DecodeError>;

    /// Append the message's present fields, in field-number order.
    fn write(&self, out: &mut Vec<u8>);

    /// The message's bytes.
    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }
}

/// A value of a field, or of one item of a repeated field: a scalar or a nested message.
pub(crate) trait Value: Sized {
    /// Read the value.
    fn read_value(reader: &mut Reader<'_>) -> Result<Self, DecodeError>;

    /// Write the value as field `number`.
    fn write_value(&self, number: u32, out: &mut Vec<u8>);
}

impl Value for u32 {
    fn read_value(reader: &mut Reader<'_>) -> Result<u32, DecodeError> {
        reader.uint32()
    }

    fn write_value(&self, number: u32, out: &mut Vec<u8>) {
        write_tag(out, number, VARINT);
        write_varint(out, *self);
    }
}

impl Value for bool {
    fn read_value(reader: &mut Reader<'_>) -> Result<bool, DecodeError> {
        reader.bool()
    }

    fn write_value(&self, number: u32, out: &mut Vec<u8>) {
        u32::from(*self).write_value(number, out);
    }
}

impl Value for String {
    fn read_value(reader: &mut Reader<'_>) -> Result<String, DecodeError> {
        reader.string()
    }

    fn write_value(&self, number: u32, out: &mut Vec<u8>) {
        write_delimited(out, number, self.as_bytes());
    }
}

impl Value for Vec<u8> {
    fn read_value(reader: &mut Reader<'_>) -> Result<Vec<u8>, DecodeError> {
        reader.bytes().map(<[u8]>::to_vec)
    }

    fn write_value(&self, number: u32, out: &mut Vec<u8>) {
        write_delimited(out, number, self);
    }
}

impl<T: Message> Value for T {
    fn read_value(reader: &mut Reader<'_>) -> Result<T, DecodeError> {
        let end = reader.nested_end()?;
        T::read(reader, end)
    }

    fn write_value(&self, number: u32, out: &mut Vec<u8>) {
        write_delimited(out, number, &self.to_bytes());
    }
}

/// How a field of each Rust type is read, written and seen as an own key.
pub(crate) trait FieldValue {
    /// Read one occurrence: a singular field takes the value (the last occurrence wins, and
    /// messages are not merged), a repeated field appends it, the map assigns its entry.
    fn read(&mut self, reader: &mut Reader<'_>) -> Result<(), DecodeError>;

    /// Write the field as field `number`, if it is present.
    fn write(&self, number: u32, out: &mut Vec<u8>);

    /// Whether the reference's object has the field as an own key: a singular field that is set,
    /// and always a repeated field or the map, which the constructor creates.
    fn present(&self) -> bool;
}

impl<T: Value> FieldValue for Option<T> {
    fn read(&mut self, reader: &mut Reader<'_>) -> Result<(), DecodeError> {
        *self = Some(T::read_value(reader)?);
        Ok(())
    }

    fn write(&self, number: u32, out: &mut Vec<u8>) {
        if let Some(value) = self {
            value.write_value(number, out);
        }
    }

    fn present(&self) -> bool {
        self.is_some()
    }
}

impl<T: Value> FieldValue for Vec<T> {
    fn read(&mut self, reader: &mut Reader<'_>) -> Result<(), DecodeError> {
        self.push(T::read_value(reader)?);
        Ok(())
    }

    fn write(&self, number: u32, out: &mut Vec<u8>) {
        for value in self {
            value.write_value(number, out);
        }
    }

    fn present(&self) -> bool {
        true
    }
}

impl FieldValue for Plugins {
    /// One map entry: its key (field 1, `""` when absent) and its value (field 2, `None` when
    /// absent), then `plugins[key] = value`.
    fn read(&mut self, reader: &mut Reader<'_>) -> Result<(), DecodeError> {
        let end = reader.nested_end()?;
        let mut name = String::new();
        let mut plugin: Option<Plugin> = None;
        reader.fields(end, |reader, number| {
            match number {
                1 => name = reader.string()?,
                2 => plugin.read(reader)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        self.insert(name, plugin);
        Ok(())
    }

    /// One entry per own key, in order, each with its key and its value. The reference's writer
    /// throws on an entry without a value; this writer leaves the value out, so that the entry
    /// reads back without one. An own key `__proto__` needs the `null` prototype first, which
    /// such an entry named `__proto__` sets.
    fn write(&self, number: u32, out: &mut Vec<u8>) {
        let null_prototype = self.has_own_proto_key().then_some((js::PROTO_KEY, None));
        for (name, plugin) in null_prototype.into_iter().chain(self.iter()) {
            let mut entry = Vec::new();
            write_delimited(&mut entry, 1, name.as_bytes());
            if let Some(plugin) = plugin {
                plugin.write_value(2, &mut entry);
            }
            write_delimited(out, number, &entry);
        }
    }

    fn present(&self) -> bool {
        true
    }
}

/// The own keys of a message: the repeated fields and the map in declaration order, then the
/// present singular fields in the order `order` lists them, then the other present singular
/// fields in declaration order.
pub(crate) fn own_keys(
    fields: &[Field],
    order: &WireOrder,
    present: impl Fn(u32) -> bool,
) -> Vec<&'static str> {
    let seen = order
        .numbers()
        .iter()
        .filter_map(|&number| fields.iter().find(|field| field.number == number));
    let unseen = fields
        .iter()
        .filter(|field| !order.numbers().contains(&field.number));
    let singular = seen
        .chain(unseen)
        .filter(|field| !field.repeated && present(field.number));
    fields
        .iter()
        .filter(|field| field.repeated)
        .chain(singular)
        .map(|field| field.name)
        .collect()
}

/// The Rust type of a field of each kind: singular fields are `Option`s (`None` is absent on the
/// wire), repeated fields and the map are always present.
macro_rules! field_type {
    (u32) => { Option<u32> };
    (bool) => { Option<bool> };
    (string) => { Option<String> };
    (bytes) => { Option<Vec<u8>> };
    (message($inner:ty)) => { Option<$inner> };
    (strings) => { Vec<String> };
    (messages($inner:ty)) => { Vec<$inner> };
    (map($inner:ty)) => { $inner };
}

/// Whether a field of this kind is repeated or the map: the reference's constructor creates it,
/// so it is an own key of every decoded object.
macro_rules! is_repeated {
    (strings) => {
        true
    };
    (messages) => {
        true
    };
    (map) => {
        true
    };
    ($kind:ident) => {
        false
    };
}

/// Declare a protobuf message: the struct with its public fields and `wire_order`, and its
/// `Proto` implementation.
macro_rules! message {
    (
        $(#[$meta:meta])*
        $name:ident {
            $(
                $(#[$field_meta:meta])*
                $number:literal $field:ident $js:literal : $kind:ident $(($inner:ty))?
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default, PartialEq, Eq)]
        pub struct $name {
            $(
                $(#[$field_meta])*
                pub $field: $crate::proto::field_type!($kind $(($inner))?),
            )*
            /// The order in which the singular fields first appeared on the wire, which is the
            /// order of the reference's own keys.
            pub wire_order: $crate::messages::WireOrder,
        }

        impl $crate::proto::Message for $name {
            fn read(
                reader: &mut $crate::proto::Reader<'_>,
                end: usize,
            ) -> Result<Self, $crate::errors::DecodeError> {
                let mut message = Self::default();
                reader.fields(end, |reader, number| {
                    match number {
                        $($number => $crate::proto::FieldValue::read(&mut message.$field, reader)?,)*
                        _ => return Ok(false),
                    }
                    message.wire_order.note(number);
                    Ok(true)
                })?;
                Ok(message)
            }

            fn write(&self, out: &mut Vec<u8>) {
                $($crate::proto::FieldValue::write(&self.$field, $number, out);)*
            }
        }

        impl $crate::messages::Proto for $name {
            const FIELDS: &'static [$crate::messages::Field] = &[
                $(
                    $crate::messages::Field {
                        number: $number,
                        name: $js,
                        repeated: $crate::proto::is_repeated!($kind),
                    },
                )*
            ];

            fn encode(&self) -> Vec<u8> {
                $crate::proto::Message::to_bytes(self)
            }

            fn decode(bytes: &[u8]) -> Result<Self, $crate::errors::DecodeError> {
                $crate::proto::Message::read(&mut $crate::proto::Reader::new(bytes), bytes.len())
            }

            fn own_keys(&self) -> Vec<&'static str> {
                $crate::proto::own_keys(Self::FIELDS, &self.wire_order, |number| match number {
                    $($number => $crate::proto::FieldValue::present(&self.$field),)*
                    _ => false,
                })
            }
        }

        #[cfg(test)]
        impl $crate::proto::tests::Arbitrary for $name {
            fn arbitrary(rng: &mut $crate::proto::tests::Rng) -> Self {
                $name {
                    $($field: $crate::proto::tests::Arbitrary::arbitrary(rng),)*
                    wire_order: $crate::messages::WireOrder::default(),
                }
            }
        }
    };
}

pub(crate) use {field_type, is_repeated, message};
