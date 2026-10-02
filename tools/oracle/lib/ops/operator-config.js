"use strict";
// The forger key file's operation: `config.forgerKey` ({flags, delegatesJson}: the reference's
// `config forger private-key`: the validation of the flag, `require` of the existing delegates.json
// (an absent file fails there), `keys` set to the lower-cased key, and `writeJsonSync(file, delegates,
// {spaces: 4})`). It gives {delegatesJson}, the text the command leaves in the file, or the error.
//
// `flags` is the object the CLI's parser hands the command (`privateKey`, a string); `delegatesJson`
// is the text of the existing file, or absent for no file. The flag is validated by the command's
// own schema (`Joi.string().hex().length(64)`) through the CLI's validator (`Joi.object(schema)
// .unknown(true)`, the messages of every failure joined), then the command's own lines run on a real
// file in a scratch directory, with the reference's own `require` and `fs-extra`. The scratch
// directory's name is replaced by `<config>` in an error's message, which is kept to its first line.
//
// Left out on purpose: the flag as `yargs-parser` reads it from a command line (`--privateKey` with
// a value such as sixty-four zeros reads as a number, which the schema refuses), and a file that is
// a JSON array (the reference writes it back without the key).

const fs = require("fs");
const os = require("os");
const path = require("path");

module.exports = function operatorConfigOps(context) {
    const { solar } = context;
    const coreDir = path.join(solar.solarDir, "packages", "core");
    const needs = (name) => require(require.resolve(name, { paths: [coreDir] }));

    return {
        "config.forgerKey": ({ flags, delegatesJson }) => {
            if (flags === null || typeof flags !== "object") throw new TypeError("flags must be an object");
            if (delegatesJson !== undefined && typeof delegatesJson !== "string") throw new TypeError("delegatesJson must be a string");
            const Joi = needs("joi");
            const { writeJsonSync } = needs("fs-extra");
            const dir = fs.mkdtempSync(path.join(os.tmpdir(), "oracle-forger-key-"));
            try {
                // The CLI's InputValidator, with the schema the command declares.
                const { error } = Joi.object({ privateKey: Joi.string().hex().length(64) }).unknown(true).validate(flags);
                if (error) throw new Error(error.details.map((detail) => detail.message).join(""));
                const file = path.join(dir, "delegates.json");
                if (delegatesJson !== undefined) fs.writeFileSync(file, delegatesJson);
                // The command's lines, `config-forger-private-key.ts`.
                const delegates = require(file);
                delegates.keys = [flags.privateKey.toLowerCase()];
                writeJsonSync(file, delegates, { spaces: 4 });
                return { delegatesJson: fs.readFileSync(file, "utf8") };
            } catch (e) {
                const message = String(e && e.message !== undefined ? e.message : e).split("\n")[0];
                const Class = e instanceof Error ? e.constructor : Error;
                throw new Class(message.split(dir).join("<config>"));
            } finally {
                delete require.cache[path.join(dir, "delegates.json")];
                fs.rmSync(dir, { recursive: true, force: true });
            }
        },
    };
};
