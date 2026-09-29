"use strict";
// Oracle operations of the chain routes' documents and of the query fuzz target's replay:
//   api.spec                {network, pool, blockchain, version} -> {document}  (www/api.json patched)
//   api.transactionSchemas  {network} -> {data}  (the activated handlers' schemas)
//   api.feeStatistics       {rows, days?} -> {meta, data}
//   api.fuzzQuery           {hex} -> {status, query?, page?}  (a fuzz input: the route's choice
//                           byte, then the raw query, through api.parseQuery, the route's
//                           validation and api.paginate)
// Not built yet: each operation throws.
// Loaded by lib/ops.js.

module.exports = function apiSpecOps() {
    return {
        "api.spec": async () => {
            throw new Error("api.spec is not built");
        },
        "api.transactionSchemas": async () => {
            throw new Error("api.transactionSchemas is not built");
        },
        "api.feeStatistics": async () => {
            throw new Error("api.feeStatistics is not built");
        },
        "api.fuzzQuery": async () => {
            throw new Error("api.fuzzQuery is not built");
        },
    };
};
