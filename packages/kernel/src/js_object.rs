//! What the kernel's JavaScript-compatible helpers know about plain JavaScript objects.

/// The own properties of `Object.prototype`. A plain object answers `in` for these names without
/// having them, which the reference's canonical JSON and its option merge both depend on.
pub(crate) const PROTOTYPE_NAMES: [&str; 12] = [
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
    "__proto__",
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
];
