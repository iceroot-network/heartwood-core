//! The line templates: each known line kind as the ordered literal pieces of the log call that
//! writes it, its level and its trailing emoji. A line matches only its whole template, its level
//! and its emoji; the message is matched before the emoji. Several messages are composed in the
//! node's source (the quorum refusals are `"{refusal}. Will not forge"`), so a template is never
//! one literal sentence. The drift guard finds every template's pieces, in order, in one log call
//! of the node's source files.

use heartwood_kernel::log::Level;

use crate::doctor::sources::logs::LineKind;
use crate::seams::Role;

/// One template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Template {
    /// The kind's name, as the MCP log summary lists it.
    pub kind: &'static str,
    /// The role whose log holds it.
    pub role: Role,
    /// The level it is logged at.
    pub level: Level,
    /// The literal pieces, in order.
    pub pieces: &'static [&'static str],
    /// The trailing emoji, when the line has one.
    pub emoji: Option<&'static str>,
    /// The node's source file that writes it, relative to the repository's root.
    pub source: &'static str,
}

/// Every template.
pub const TEMPLATES: &[Template] = &[];

/// The kind of a `role`'s line at `level` with the message `message` (its emoji included), or
/// `None` for another line.
pub fn classify(role: Role, level: Level, message: &str) -> Option<LineKind> {
    let _ = (role, level, message);
    None
}
