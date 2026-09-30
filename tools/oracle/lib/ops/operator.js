"use strict";
// The environment file's operations: `env.stringify` ({vars, file}: the text the reference's envfile
// writes, `stringifySync(env).trim()`, for the file's variables with `vars` assigned) and `env.parseCli` ({text}: the variables the reference's
// CLI reads with envfile's parser, in the file's order, and the note `cliEnvParser` where the
// node's own parser reads a line otherwise).
//
// `vars` is a list of [key, value] pairs, each a string or a number, assigned in turn as
// `env[key] = value` to the variables of `file` (the text of the environment file, read with
// envfile's parser; none: an empty object), as the reference's `env:set` does. The reference's
// CLI package holds envfile; its own `Utils.dotenv` is the node's parser. Both operations note
// `cliEnvParser` when envfile and the node's parser read the text differently.

const assert = require("assert");
const path = require("path");

module.exports = function operatorOps(context) {
    const { solar } = context;
    const cliDir = path.join(solar.solarDir, "packages", "cli");
    const kernelDir = path.join(solar.solarDir, "packages", "kernel");
    const envfile = () => require(require.resolve("envfile", { paths: [cliDir] }));
    const nodeParser = () => require(path.join(kernelDir, "dist", "utils", "dot-env.js")).dotenv;

    // envfile's variables of `text` and the node's, each as [key, value] pairs, and whether the two
    // read it alike (the node's values as its environment stores them).
    function read(text) {
        const cli = Object.entries(envfile().parseSync(text));
        const node = Object.entries(nodeParser().parse(text)).map(([key, value]) => [key, String(value)]);
        let same = true;
        try {
            assert.deepStrictEqual(cli, node);
        } catch {
            same = false;
        }
        return { cli, notes: { cliEnvParser: !same } };
    }

    return {
        "env.stringify": ({ vars, file }) => {
            if (!Array.isArray(vars)) throw new TypeError("vars must be an array of [key, value] pairs");
            if (file !== undefined && typeof file !== "string") throw new TypeError("file must be a string");
            const env = file === undefined ? {} : envfile().parseSync(file);
            for (const pair of vars) {
                if (!Array.isArray(pair) || pair.length !== 2 || typeof pair[0] !== "string") throw new TypeError("a variable must be [key, value] with a string key");
                if (typeof pair[1] !== "string" && typeof pair[1] !== "number") throw new TypeError("a value must be a string or a number");
                env[pair[0]] = pair[1];
            }
            const notes = file === undefined ? { cliEnvParser: false } : read(file).notes;
            return { text: envfile().stringifySync(env).trim(), notes };
        },

        "env.parseCli": ({ text }) => {
            if (typeof text !== "string") throw new TypeError("text must be a string");
            const { cli, notes } = read(text);
            return { pairs: cli, notes };
        },
    };
};
