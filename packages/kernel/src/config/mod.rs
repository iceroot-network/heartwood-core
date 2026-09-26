//! Configuration helpers with the reference's semantics: the `.env` parser and the option merge.
//!
//! Loading `app.json` and the component options is not here yet: it arrives with the first
//! component that reads them.

pub mod dot_env;
pub mod merge;
