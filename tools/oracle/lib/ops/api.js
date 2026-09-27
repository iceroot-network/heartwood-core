"use strict";
// Oracle operations of the API's query grammar and pagination:
//   api.parseQuery {query}               -> the parsed object: the reference server's qs parse,
//                                           then its comma-array and dot-nesting plugins
//   api.paginate   {path, query, total}  -> the list's meta: the pagination extension's links
// Loaded by lib/ops.js. Until an operation is written it throws, and the harness reports its
// records as having no Rust implementation.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = function apiOps() {
    return {
        "api.parseQuery": notWritten("api.parseQuery"),
        "api.paginate": notWritten("api.paginate"),
    };
};
