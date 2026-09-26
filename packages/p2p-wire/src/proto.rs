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
                todo!()
            }

            fn decode(bytes: &[u8]) -> Result<Self, $crate::errors::DecodeError> {
                let _ = bytes;
                todo!()
            }

            fn own_keys(&self) -> Vec<&'static str> {
                todo!()
            }
        }
    };
}

pub(crate) use {field_type, is_repeated, message};
