//! The canonical JSON of the status tree: the kernel's `stringify`, which passes the sorted list
//! of every key in the tree to `JSON.stringify` as its replacer. For the status tree this is JSON
//! with the keys of every object sorted by UTF-16 code units, no white space, strings escaped as
//! `JSON.stringify` escapes them, integers as decimal text, and keys whose value is `undefined`
//! left out.
