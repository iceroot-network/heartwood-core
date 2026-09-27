//! Configuration helpers with the reference's semantics: the `.env` parser, the option merge, and
//! the crypto files of a network with their strict schema.
//!
//! Loading `app.json` and the component options is not here yet: it arrives with the first
//! component that reads them.

pub mod crypto;
pub mod dot_env;
pub mod merge;
pub mod schema;

pub use self::crypto::{CryptoFiles, CryptoFilesError, CryptoTexts};
pub use self::schema::SchemaError;
