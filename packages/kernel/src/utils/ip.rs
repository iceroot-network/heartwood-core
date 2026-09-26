//! IP addresses and address lists.
//!
//! Whitelists and blacklists match each entry as a glob against the address text, with the
//! subset of the reference's glob matcher that address lists use (`*`, `?`, `[...]`); an entry
//! that is not a valid glob matches nothing. An empty whitelist allows everyone and an empty
//! blacklist bans no one. Remote access lists instead match exact addresses, and an empty one
//! allows no one.

/// Whether `ip` (brackets removed) is a valid IPv4 or IPv6 address.
pub fn is_valid_address(ip: &str) -> bool {
    let _ = ip;
    todo!()
}

/// Whether `ip` (brackets removed) is an IPv6 address.
pub fn is_ipv6_address(ip: &str) -> bool {
    let _ = ip;
    todo!()
}

/// `ip` without brackets, then in brackets if it is an IPv6 address.
pub fn normalise_address(ip: &str) -> String {
    let _ = ip;
    todo!()
}

/// `ip` with the first `[` and the first `]` removed.
pub fn clean_address(ip: &str) -> String {
    let _ = ip;
    todo!()
}

/// Whether `remote_address` matches an entry of `whitelist`, or the whitelist is empty.
pub fn is_whitelisted(whitelist: &[String], remote_address: &str) -> bool {
    let _ = (whitelist, remote_address);
    todo!()
}

/// Whether `remote_address` matches an entry of `blacklist`.
pub fn is_blacklisted(blacklist: &[String], remote_address: &str) -> bool {
    let _ = (blacklist, remote_address);
    todo!()
}

/// Whether `remote_address` is one of the addresses of `allowed`, compared after normalisation;
/// an empty list allows no one.
pub fn is_allowed_exactly(allowed: &[String], remote_address: &str) -> bool {
    let _ = (allowed, remote_address);
    todo!()
}
