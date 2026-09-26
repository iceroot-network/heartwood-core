"use strict";
// Load a built Solar Core reference (the TypeScript oracle) and describe where it came from.
//
// The reference is Solar Core 4.3.1 plus a patch series, from iceroot-network/solar-core-ref at a
// given commit (by default the tag s1-ref-v1), installed and built with pnpm. Inside the oracle
// image it lives at /opt/solar and the image sets SOLAR_DIR to it; locally it is any built checkout,
// passed as --solar-dir or SOLAR_DIR.

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

function sha256Hex(data) {
    return crypto.createHash("sha256").update(data).digest("hex");
}

function readTrimmed(file) {
    try {
        return fs.readFileSync(file, "utf8").trim();
    } catch {
        return null;
    }
}

/**
 * Where the Solar sources came from. The image build writes .solar-commit and .solar-tree after it
 * has checked the tarball's commit and the extracted tree hash; a local checkout asks git.
 */
function provenance(solarDir) {
    const commit = readTrimmed(path.join(solarDir, ".solar-commit"));
    if (commit) {
        return { commit, tree: readTrimmed(path.join(solarDir, ".solar-tree")), dirty: false, source: "image" };
    }
    const git = (...args) => spawnSync("git", ["-C", solarDir, ...args], { encoding: "utf8" });
    const head = git("rev-parse", "HEAD");
    if (head.status !== 0) return { commit: null, tree: null, dirty: null, source: "unknown" };
    return {
        commit: head.stdout.trim(),
        tree: git("rev-parse", "HEAD^{tree}").stdout.trim(),
        dirty: git("status", "--porcelain").stdout.trim() !== "",
        source: "git",
    };
}

/** Load @solar-network/crypto (and bcrypto and bip39 as it resolves them) from a built reference. */
function loadSolar(dir) {
    const given = dir || process.env.SOLAR_DIR;
    if (!given) {
        throw new Error("no Solar reference: pass --solar-dir DIR or set SOLAR_DIR to a built checkout of the reference");
    }
    const solarDir = path.resolve(given);
    const cryptoDir = path.join(solarDir, "packages", "crypto");
    const distIndex = path.join(cryptoDir, "dist", "index.js");
    if (!fs.existsSync(distIndex)) {
        throw new Error(`no built @solar-network/crypto at ${distIndex}; pass --solar-dir <built Solar reference>`);
    }
    const version = JSON.parse(fs.readFileSync(path.join(cryptoDir, "package.json"), "utf8")).version;
    const lib = require(distIndex);
    const bcrypto = require(require.resolve("bcrypto", { paths: [cryptoDir] }));
    const bip39 = require(require.resolve("bip39", { paths: [cryptoDir] }));
    const seriesFile = path.join(solarDir, "patches", "series", "SERIES.md");
    return {
        solarDir,
        cryptoDir,
        version,
        lib,
        bip39,
        // bcrypto 5.4.0: native === 2 when the compiled backend loaded, 0 under NODE_BACKEND=js.
        bcryptoBackend: bcrypto.native === 2 ? "native" : "js",
        provenance: provenance(solarDir),
        seriesSha256: fs.existsSync(seriesFile) ? sha256Hex(fs.readFileSync(seriesFile)) : null,
    };
}

/** Solar 4.3.1's native modules only build and load on Node 18. */
function assertNode18() {
    if (!process.version.startsWith("v18.")) {
        throw new Error(`the Solar reference needs Node 18 (18.20.8 is pinned); this is ${process.version}`);
    }
}

module.exports = { assertNode18, loadSolar, sha256Hex };
