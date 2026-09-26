#!/usr/bin/env node
"use strict";
// oracle-server.js: the oracle for level L1 of the differential tests (random and mutated cases,
// one oracle call each). JSON lines over stdio.
//
//   node oracle-server.js [--solar-dir DIR]
//
//   -> {"id":17,"op":"tx.fromBytes","network":"devnet","height":2,"args":{"hex":"ff03...","strict":true}}
//   <- {"id":17,"ok":true,"result":{...}}
//   <- {"id":18,"ok":false,"error":{"class":"TransactionSchemaError","message":"..."}}
//
// The operations are those of lib/ops.js, the same code that writes the L0 vectors. Every request
// sets its height first. Only the devnet network is served; `ping` needs no network or height.
// The process never opens a socket: in the image it runs with --network none.

const readline = require("readline");

const { assertNode18, loadSolar } = require("./lib/solar");
const { buildDevnet } = require("./lib/network");
const { errorInfo, makeOps } = require("./lib/ops");

function parseArgs(argv) {
    const opts = { solarDir: undefined };
    for (let i = 0; i < argv.length; i++) {
        if (argv[i] === "--solar-dir" && i + 1 < argv.length) opts.solarDir = argv[++i];
        else throw new Error(`unknown argument ${argv[i]}`);
    }
    return opts;
}

function main() {
    const opts = parseArgs(process.argv.slice(2));
    assertNode18();
    const solar = loadSolar(opts.solarDir);
    const devnet = buildDevnet(solar);
    const { ops, run } = makeOps(solar, devnet);

    const respond = (value) => process.stdout.write(JSON.stringify(value) + "\n");
    const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
    rl.on("line", (line) => {
        if (line.trim() === "") return;
        let request;
        try {
            request = JSON.parse(line);
        } catch (error) {
            respond({ id: null, ok: false, error: { class: "RequestError", message: `not JSON: ${error.message}` } });
            return;
        }
        const { id = null, op, network, height, args } = request;
        try {
            if (op === "ping") {
                respond({ id, ok: true, result: ops.ping() });
                return;
            }
            if (!Object.prototype.hasOwnProperty.call(ops, op)) throw new Error(`unknown operation ${op}`);
            if (network !== devnet.info.name) throw new Error(`network ${network} is not served (only ${devnet.info.name})`);
            if (!Number.isSafeInteger(height) || height < 1) throw new Error("height must be a positive integer");
            const result = run(op, height, args);
            respond(result.error ? { id, ok: false, error: result.error } : { id, ok: true, result: result.output });
        } catch (error) {
            respond({ id, ok: false, error: { ...errorInfo(error), class: "RequestError" } });
        }
    });
}

try {
    main();
} catch (error) {
    console.error(`oracle-server: ${error.message}`);
    process.exit(1);
}
