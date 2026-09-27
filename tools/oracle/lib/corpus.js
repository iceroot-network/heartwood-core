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
//   reference/restarts.jsonl        the node's live restarts, optional: {height, path, checkpoint},
//                                   path "saved" or "rebuild" (how the node started again), and
//                                   checkpoint the file of the state right after it: an API
//                                   checkpoint, or a snapshot (reference/restarts/h<H>.json, the
//                                   devnet tooling's snapshot format)
//   revert-plan.json                {seed, roundLength, tip, steps: [{after, blocks}]}
//   restart-plan.json               the restarts the judge and the L2 replay run, optional:
//                                   {kind, seed, roundLength, tip, live, heights, reverts, modes}
//   corpus.json                     provenance, heights and counts
//   ts-digests.jsonl, anchors.json  written by replay-chain.js
//   ts-restarts.jsonl               written by replay-chain.js with a restart plan
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
    const restartPlanFile = path.join(dir, "restart-plan.json");
    const restarts = readJsonl(path.join(dir, "reference", "restarts.jsonl"), true).map((r, i) => {
        if (!Number.isSafeInteger(r.height) || (r.path !== "saved" && r.path !== "rebuild") || typeof r.checkpoint !== "string" || !/^reference\/[a-z]+\/h\d+\.json$/.test(r.checkpoint)) {
            throw new Error(`${dir}/reference/restarts.jsonl: line ${i + 1} is not {height, path, checkpoint}`);
        }
        return { ...r, data: readJson(path.join(dir, r.checkpoint)) };
    });
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
            restarts,
        },
        revertPlan: fs.existsSync(planFile) ? readJson(planFile) : { steps: [] },
        restartPlan: fs.existsSync(restartPlanFile) ? readJson(restartPlanFile) : undefined,
    };
}

/**
 * `corpus` cut at height `to`: the blocks up to it, the reverts that run after them and revert
 * no more than they hold, and the node's rows, block columns, checkpoints and live restarts up to
 * it. `lastRound` is the last round whose rows a replay to `to` saves: the round of `to`, or the
 * next round when `to` ends a round.
 */
function cutCorpus(corpus, to, lastRound) {
    if (!Number.isSafeInteger(to) || to < 1) throw new TypeError("the cut must be a height");
    if (to > corpus.blocks.length + 1) throw new RangeError(`the corpus ends at ${corpus.blocks.length + 1}, below the cut ${to}`);
    const reference = corpus.reference;
    return {
        ...corpus,
        blocks: corpus.blocks.filter((b) => b.h <= to),
        reference: {
            ...reference,
            meta: reference.meta.filter((m) => m.h <= to),
            rounds: reference.rounds.filter((r) => r.round <= lastRound),
            missed: reference.missed.filter((r) => r.height <= to),
            rejections: reference.rejections.filter((r) => typeof r.height !== "number" || r.height <= to),
            checkpoints: reference.checkpoints.filter((c) => c.data.height <= to),
            restarts: reference.restarts.filter((r) => r.height <= to),
        },
        revertPlan: { ...corpus.revertPlan, steps: (corpus.revertPlan.steps || []).filter((s) => s.after <= to) },
        cut: to,
    };
}

module.exports = { FORMAT, CRYPTO_FILES, cutCorpus, loadCorpus, readJson, readJsonl };

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
        const out = { accepted: result.accepted, missed: result.missed, reward: result.reward, block: result.block };
        if (result.block) {
            out.height = result.block.data.height;
            out.id = result.block.data.id;
        }
        if (result.accepted) {
            out.line = this.emit(step, result.reward, result.missed);
        } else {
            out.stage = result.stage;
            out.error = { class: result.error.constructor ? result.error.constructor.name : "Error", message: String(result.error.message) };
        }
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

    /**
     * Stop the node at its tip and start it again (HeadlessChain.restart), then continue on the
     * started node. Returns {line} with the restart's digest line (step "restart", no missed
     * slots), or {refused: {reason}}; the session then keeps its node. `notes.savedStateUsername`
     * is true for a restart by the saved state when a delegate's name or a voted name reads as an
     * integer (the saved-state reviver turns it into a number, or cannot load the file).
     */
    async restart(mode) {
        const notes = {};
        if (mode === "saved" && integerLikeNames(this.chain.wallets()).length) notes.savedStateUsername = true;
        const result = await this.chain.restart(mode);
        const out = Object.keys(notes).length ? { notes } : {};
        if (result.refused) return { ...out, refused: result.refused };
        this.chain = result.chain;
        return { ...out, line: this.emit("restart", undefined, []) };
    }

    /** The sections of the current state, with the missed rows of the last step. */
    sections() {
        const { chainSections } = require("./digest");
        return chainSections(this.chain, this.lastMissed);
    }
}

/**
 * Whether the saved-state loader's reviver turns the string `value` into a number: text without a
 * dot that reads as an integer.
 */
function revivedAsNumber(value) {
    return typeof value === "string" && !value.includes(".") && !isNaN(+value) && Number.isInteger(+value);
}

/** The delegate names and voted names among `wallets` that the saved-state reviver turns into numbers. */
function integerLikeNames(wallets) {
    const names = new Set();
    for (const wallet of wallets) {
        if (wallet.hasAttribute("delegate.username") && revivedAsNumber(wallet.getAttribute("delegate.username"))) {
            names.add(wallet.getAttribute("delegate.username"));
        }
        if (wallet.hasAttribute("votes")) {
            for (const name of wallet.getAttribute("votes").keys()) if (revivedAsNumber(name)) names.add(name);
        }
    }
    return [...names];
}

module.exports.ReplaySession = ReplaySession;
module.exports.integerLikeNames = integerLikeNames;
