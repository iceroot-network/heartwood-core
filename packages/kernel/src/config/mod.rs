//! Configuration with the reference's semantics: the `.env` parser and the node's environment, the
//! option merge, `app.json` and the components' options, `peers.json`, the crypto files of a
//! network with their strict schema, and the version floor of the node's own version.

pub mod app;
pub mod crypto;
pub mod dot_env;
pub mod env;
pub mod floor;
pub mod merge;
pub mod peers;
pub mod schema;

pub use self::app::{AppConfig, AppConfigError, Options, OptionsError, PluginEntry, Role};
pub use self::crypto::{CryptoFiles, CryptoFilesError, CryptoTexts};
pub use self::env::{Env, EnvError};
pub use self::floor::{FloorError, FloorWarning};
pub use self::peers::{PeerData, PeersError, PeersFile};
pub use self::schema::{KeyProblem, SchemaError};
