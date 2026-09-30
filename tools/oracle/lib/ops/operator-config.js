"use strict";
// The forger key file's operation: `config.forgerKey` ({flags, delegatesJson}: the reference's
// validation of the key, its failure on an absent delegates.json, and the file it writes).

module.exports = function operatorConfigOps(_context) {
    return {
        "config.forgerKey": async () => {
            throw new Error("config.forgerKey is not built");
        },
    };
};
