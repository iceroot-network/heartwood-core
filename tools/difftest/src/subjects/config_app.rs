//! The node's configuration: `config.pluginOptions` (`{package, options, env}` to the options after
//! the package's schema and defaults, or the error) and `config.loadApp` (`app.json` text and a
//! role to the profile's components, or the error), answered with `heartwood_kernel::config` and
//! the components' options types.

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of the node's configuration.
pub(super) const OPS: &[(&str, Run)] = &[
    ("config.pluginOptions", plugin_options),
    ("config.loadApp", load_app),
];

fn plugin_options(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}

fn load_app(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = (env, case);
    Err(SubjectError::NotImplemented)
}
