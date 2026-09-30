"use strict";
// The environment file's operations: `env.stringify` ({vars}: the text the reference's envfile
// writes, `stringifySync(vars).trim()`) and `env.parseCli` ({text}: the variables the reference's
// CLI reads with envfile's parser, in the file's order, and the note `cliEnvParser` where the
// node's own parser reads a line otherwise).

module.exports = function operatorOps(_context) {
    const notBuilt = (op) => async () => {
        throw new Error(`${op} is not built`);
    };
    return {
        "env.stringify": notBuilt("env.stringify"),
        "env.parseCli": notBuilt("env.parseCli"),
    };
};
