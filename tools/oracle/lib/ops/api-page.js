"use strict";
// Oracle operations of the kernel's search services and functional-red-black-tree 1.0.1:
//   api.searchPage  {items, criteria, sorting, pagination} -> {page} or {thrown: {class, message}}
//   js.rbTrace      {script, compare} -> {compared: [[i, j], ...], keys}
// Items carry BigNumber and Semver markers; `compared` lists the index pairs the tree compares, in
// order.
// Not built yet: each operation throws.
// Loaded by lib/ops.js.

module.exports = function apiPageOps() {
    return {
        "api.searchPage": async () => {
            throw new Error("api.searchPage is not built");
        },
        "js.rbTrace": async () => {
            throw new Error("js.rbTrace is not built");
        },
    };
};
