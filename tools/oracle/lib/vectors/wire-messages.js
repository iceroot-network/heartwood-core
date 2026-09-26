"use strict";
// V13, protobuf messages: each message with all fields present, all absent and present with
// default values; varints of 5 and 10 bytes; repeated, unknown and mistyped fields; groups,
// once nested 50,000 deep; overruns; map entries without key or value, repeated names,
// array-index names and __proto__. Returns its records, or null while it has none.

module.exports = async function wireMessagesVectors(ctx) {
    void ctx;
    return null;
};
