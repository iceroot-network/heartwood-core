#!/usr/bin/env node
"use strict";
// check-native.js: fail the image build if Solar 4.3.1's native modules did not build or load.
//
// Run in the build stage (after `pnpm build`) and again in the runtime stage (which only copies
// /opt/solar), so a missing shared library in the slim runtime image is caught at build time.
// Each module is resolved from the Solar package that depends on it (pnpm's strict layout).
//
// Usage: node check-native.js [/opt/solar]

const path = require("path");

const solarDir = path.resolve(process.argv[2] || "/opt/solar");
const wanted = [
    // [package dir, module, check]
    ["packages/crypto", "bcrypto", (m) => m.native === 2 || `bcrypto fell back to its JS backend (native=${m.native})`],
    ["packages/pool", "better-sqlite3", (Database) => {
        const db = new Database(":memory:");
        const row = db.prepare("select sqlite_version() as v").get();
        db.close();
        return typeof row.v === "string" || "better-sqlite3 returned no version";
    }],
    ["packages/cli", "node-pty", (m) => typeof m.spawn === "function" || "node-pty has no spawn()"],
    ["packages/kernel", "nsfw", (m) => typeof m === "function" || "nsfw is not a function"],
    ["packages/core", "argon2", (m) => typeof m.hash === "function" || "argon2 has no hash()"],
];

if (!process.version.startsWith("v18.")) {
    console.error(`check-native: Solar 4.3.1 needs Node 18, this is ${process.version}`);
    process.exit(1);
}

let failed = 0;
for (const [dir, name, check] of wanted) {
    try {
        const resolved = require.resolve(name, { paths: [path.join(solarDir, dir)] });
        const result = check(require(resolved));
        if (result !== true) throw new Error(result);
        console.log(`check-native: ok   ${name} (${path.relative(solarDir, resolved)})`);
    } catch (error) {
        failed++;
        console.error(`check-native: FAIL ${name} from ${dir}: ${error.message}`);
    }
}
console.log(`check-native: node ${process.version} ${process.platform}/${process.arch}`);
process.exit(failed ? 1 : 0);
