"use strict";
// A corpus of the L2 replay: a chain produced by a reference devnet, in Heartwood's corpus format
// (written by `difftest corpus import`, format heartwood-corpus/1):
//
//   crypto/{network,milestones,genesisBlock,exceptions}.json   the chain definition
//   blocks.jsonl                    {h, id, hex} per block after the genesis block; hex is the full
//                                   block (header with signature, u32le transaction lengths, the
//                                   transactions), as a relay receives it
//   reference/meta.jsonl            {h, username, burnedFee, donations} per block: the node's own
//                                   block columns
//   reference/rounds.jsonl          {round, publicKey, balance}: the node's round rows
//   reference/missed-blocks.jsonl   {timestamp, height, username}: the node's missed-slot rows
//   reference/rejections.jsonl      blocks the node refused or reverted (fault chains), optional
//   reference/checkpoints/h<H>.json the node's REST API state at height H
//   revert-plan.json                {seed, roundLength, tip, steps: [{after, blocks}]}
//   corpus.json                     provenance, heights and counts
//   ts-digests.jsonl, anchors.json  written by replay-chain.js
//   MANIFEST.sha256                 sha256sum lines of every other file

const fs = require("fs");
const path = require("path");

const FORMAT = "heartwood-corpus/1";
const CRYPTO_FILES = ["network", "milestones", "genesisBlock", "exceptions"];

function readJson(file) {
    return JSON.parse(fs.readFileSync(file, "utf8"));
}

function readJsonl(file, optional = false) {
    if (optional && !fs.existsSync(file)) return [];
    return fs
        .readFileSync(file, "utf8")
        .split("\n")
        .filter((line) => line.trim() !== "")
        .map((line, i) => {
            try {
                return JSON.parse(line);
            } catch (error) {
                throw new Error(`${file}: line ${i + 1} is not JSON: ${error.message}`);
            }
        });
}

/** Load a corpus directory. Throws when a required file is missing or malformed. */
function loadCorpus(dir) {
    const meta = readJson(path.join(dir, "corpus.json"));
    if (meta.format !== FORMAT) throw new Error(`${dir}: corpus format ${meta.format}, expected ${FORMAT}`);
    const crypto = {};
    for (const name of CRYPTO_FILES) crypto[name] = readJson(path.join(dir, "crypto", `${name}.json`));
    const blocks = readJsonl(path.join(dir, "blocks.jsonl"));
    blocks.forEach((b, i) => {
        if (b.h !== i + 2) throw new Error(`${dir}/blocks.jsonl: line ${i + 1} has height ${b.h}, expected ${i + 2}`);
        if (typeof b.hex !== "string" || !/^([0-9a-f]{2})+$/.test(b.hex)) throw new Error(`${dir}/blocks.jsonl: block ${b.h} has no hex`);
    });
    const checkpointDir = path.join(dir, "reference", "checkpoints");
    const checkpoints = fs.existsSync(checkpointDir)
        ? fs
              .readdirSync(checkpointDir)
              .filter((f) => /^h\d+\.json$/.test(f))
              .sort()
              .map((f) => ({ file: `reference/checkpoints/${f}`, data: readJson(path.join(checkpointDir, f)) }))
        : [];
    const planFile = path.join(dir, "revert-plan.json");
    return {
        dir,
        meta,
        crypto,
        blocks,
        reference: {
            meta: readJsonl(path.join(dir, "reference", "meta.jsonl")),
            rounds: readJsonl(path.join(dir, "reference", "rounds.jsonl")),
            missed: readJsonl(path.join(dir, "reference", "missed-blocks.jsonl")),
            rejections: readJsonl(path.join(dir, "reference", "rejections.jsonl"), true),
            checkpoints,
        },
        revertPlan: fs.existsSync(planFile) ? readJson(planFile) : { steps: [] },
    };
}

module.exports = { FORMAT, CRYPTO_FILES, loadCorpus, readJson, readJsonl };

/**
 * A replay of a chain on the reference's headless state, with a digest line after every step. The
 * configuration manager must hold the chain's configuration while its methods run (the caller
 * wraps them in chain.withConfig when other configurations share the process).
 */
class ReplaySession {
    constructor(solar, crypto) {
        const { HeadlessChain } = require("./state");
        const { DigestChain } = require("./digest");
        this.solar = solar;
        this.chain = new HeadlessChain(solar, crypto);
        this.digests = new DigestChain();
        this.lines = [];
        this.lastMissed = [];
    }

    emit(step, reward, missed) {
        const { chainSections } = require("./digest");
        const last = this.chain.lastBlock();
        this.lastMissed = missed;
        const line = this.digests.line({ height: last.data.height, step, id: last.data.id, reward }, chainSections(this.chain, missed));
        this.lines.push(line);
        return line;
    }

    /** The first start; returns the genesis line. */
    async start() {
        this.chain.configure();
        await this.chain.start();
        return this.emit("genesis", undefined, []);
    }

    /**
     * Apply block bytes as `step` (apply or reapply). An accepted block gives a line; a refused one
     * gives the error's class and message, and no line.
     */
    async apply(bytes, step = "apply") {
        const result = await this.chain.apply(bytes, step);
        const out = { accepted: result.accepted, height: result.block.data.height, id: result.block.data.id, reward: result.reward, missed: result.missed, block: result.block };
        if (result.accepted) out.line = this.emit(step, result.reward, result.missed);
        else out.error = { class: result.error.constructor ? result.error.constructor.name : "Error", message: String(result.error.message) };
        return out;
    }

    /** Revert `count` blocks; one revert line per block. */
    async revert(count) {
        const lines = [];
        await this.chain.revert(count, async () => {
            lines.push(this.emit("revert", undefined, []));
        });
        return lines;
    }

    /** The sections of the current state, with the missed rows of the last step. */
    sections() {
        const { chainSections } = require("./digest");
        return chainSections(this.chain, this.lastMissed);
    }
}

module.exports.ReplaySession = ReplaySession;
