#!/usr/bin/env node
"use strict";
// replay-chain.js: the L2 judge. It replays a corpus chain on the reference implementation's own
// state machine, run headless in this process (lib/state.js), follows the corpus's revert plan,
// writes one state digest line per step (lib/digest.js), and checks that the headless replay is
// the live node's chain (the anchors).
//
//   node replay-chain.js --corpus DIR --out FILE [--anchors FILE] [--revert-plan FILE]
//                        [--restart-plan FILE] [--restarts FILE] [--to H] [--processor]
//                        [--dump H,...] [--dump-dir DIR] [--solar-dir DIR]
//
// --out        the digest lines, one JSON object per step (ts-digests.jsonl)
// --anchors    the anchor results [anchors.json next to --out]
// --revert-plan  a revert plan instead of the corpus's revert-plan.json
// --restart-plan  a restart plan instead of the corpus's restart-plan.json (written by difftest's
//              corpus judge; without either, no restart runs): the judge also replays
//              the chain once per restart mode of the reference ("rebuild", "saved"), restarting the
//              node at every planned height (lib/state.js) and continuing on the restarted node;
//              after a restart at a round end it also reverts the plan's reverts and applies the
//              blocks again. Each restart's line goes to --restarts, and every line of these runs
//              is compared with the continuous replay's line of the same step
// --restarts   the restart lines [ts-restarts.jsonl next to --out]: {mode, h, step: "restart",
//              id, b, s, r, m, w, sd, cd}, or {mode, h, step: "restart", refused: {reason}}, with
//              notes when the oracle saw a documented difference (savedStateUsername)
// --to         judge the chain cut at height H: the blocks, reverts, rows and checkpoints up to it
// --processor  also replay the chain through the reference's process-blocks job, one block per
//              job (lib/ops/job.js), and write each block's result beside its digest line to
//              ts-processor.jsonl next to --out; the judge fails when a block is not accepted or
//              a line differs from the plain replay's
// --dump       heights whose forward-step records (the five sections) are written to --dump-dir
//              [the directory of --out] as dump-h<H>.json
//
// Anchors (a headless replay that fails one is not a judge):
//   A1  round rows: the node's rounds table equals the headless saveRound rows, per round, as sets
//       of (publicKey, balance)
//   A2  missed rows: the node's missed_blocks rows equal the rows the headless forward replay
//       recorded, as multisets of (timestamp, height, username); rows of the node at a height
//       where it refused or reverted a block (reference/rejections.jsonl) are left out, and listed
//   A3  block columns: the node's username, burned fee and donations of every block equal the
//       headless block's
//   A4  checkpoints: every API checkpoint equals the headless state at its height, projected to
//       the API's shape (balances, nonces, keys, delegate attributes, votes and shares), without
//       the node-local version, the productivity and missed-block counts
//   A5  the archives' own strict decode (verify.json) passed for every block
//   A6  live restarts (with a restart plan, when reference/restarts.jsonl lists any): the state
//       right after the judge's restart by the node's path at the node's restart height equals the
//       node's state right after its own restart (an API checkpoint, or a snapshot)
// With a restart plan the report also has `restarts`: per mode the restarts, the refused ones, the
// lines compared and the differences: a restart line must equal the continuous line of its
// height in b, s, r and w, and every later line the continuous line of the same step (a revert
// line the forward line of its height in b, s, r and w).
// Exit status: 0 when every block applied and every anchor holds, 1 otherwise, 2 on a usage error.

const fs = require("fs");
const path = require("path");

const { assertNode18, loadSolar } = require("./lib/solar");
const { cutCorpus, loadCorpus, readJson, ReplaySession } = require("./lib/corpus");

function parseArgs(argv) {
    const opts = {
        corpus: undefined,
        out: undefined,
        anchors: undefined,
        revertPlan: undefined,
        restartPlan: undefined,
        restarts: undefined,
        to: undefined,
        dump: [],
        dumpDir: undefined,
        solarDir: undefined,
        processor: false,
    };
    for (let i = 0; i < argv.length; i++) {
        const a = argv[i];
        const value = () => {
            if (i + 1 >= argv.length) throw new UsageError(`${a} needs a value`);
            return argv[++i];
        };
        if (a === "--corpus") opts.corpus = value();
        else if (a === "--out") opts.out = value();
        else if (a === "--anchors") opts.anchors = value();
        else if (a === "--revert-plan") opts.revertPlan = value();
        else if (a === "--restart-plan") opts.restartPlan = value();
        else if (a === "--restarts") opts.restarts = value();
        else if (a === "--to") opts.to = Number(value());
        else if (a === "--dump") opts.dump = value().split(",").filter(Boolean).map(Number);
        else if (a === "--dump-dir") opts.dumpDir = value();
        else if (a === "--solar-dir") opts.solarDir = value();
        else if (a === "--processor") opts.processor = true;
        else throw new UsageError(`unknown argument ${a}`);
    }
    if (!opts.corpus || !opts.out) {
        throw new UsageError(
            "usage: replay-chain.js --corpus DIR --out FILE [--anchors FILE] [--revert-plan FILE] [--restart-plan FILE] [--restarts FILE] [--to H] [--processor] [--dump H,...] [--dump-dir DIR] [--solar-dir DIR]",
        );
    }
    if (opts.dump.some((h) => !Number.isSafeInteger(h) || h < 1)) throw new UsageError("--dump takes heights");
    if (opts.to !== undefined && (!Number.isSafeInteger(opts.to) || opts.to < 1)) throw new UsageError("--to takes a height");
    return opts;
}

class UsageError extends Error {}

const LIMIT = 50; // differences listed per anchor

function amountText(value) {
    if (value === undefined || value === null) return undefined;
    return typeof value === "string" ? value : typeof value === "number" ? String(value) : value.toFixed();
}

function sameJson(a, b) {
    return JSON.stringify(a) === JSON.stringify(b);
}

/** A1: the node's round rows against the headless ones. */
function anchorRounds(corpus, store) {
    const node = new Map();
    for (const row of corpus.reference.rounds) {
        if (!node.has(row.round)) node.set(row.round, []);
        node.get(row.round).push(`${row.publicKey}:${row.balance}`);
    }
    const headless = new Map();
    for (const [round, rows] of store.rounds) headless.set(round, rows.map((r) => `${r.publicKey}:${r.balance}`));
    const differences = [];
    const rounds = [...new Set([...node.keys(), ...headless.keys()])].sort((a, b) => a - b);
    let rows = 0;
    for (const round of rounds) {
        const a = (node.get(round) || []).slice().sort();
        const b = (headless.get(round) || []).slice().sort();
        rows += a.length;
        if (!sameJson(a, b)) differences.push({ round, node: a.filter((x) => !b.includes(x)), headless: b.filter((x) => !a.includes(x)) });
    }
    return { ok: differences.length === 0, rounds: rounds.length, rows, differences: differences.slice(0, LIMIT) };
}

/** A2: the node's missed-slot rows against the headless forward replay's. */
function anchorMissed(corpus, store) {
    const key = (r) => `${r.timestamp}:${r.height}:${r.username}`;
    const forward = store.recorded.filter((r) => r.step === "start" || (r.step && r.step.startsWith("apply:")));
    const counts = new Map();
    for (const r of forward) counts.set(key(r), (counts.get(key(r)) || 0) + 1);
    const refusedHeights = new Set();
    let refusedWithoutHeight = 0;
    for (const r of corpus.reference.rejections) {
        if (typeof r.height === "number") refusedHeights.add(r.height);
        else refusedWithoutHeight++;
    }
    const excluded = [];
    const onlyNode = [];
    for (const r of corpus.reference.missed) {
        const k = key(r);
        if (counts.get(k)) {
            counts.set(k, counts.get(k) - 1);
        } else if (refusedHeights.has(r.height)) {
            excluded.push({ ...r, reason: "the node refused or reverted a block at this height" });
        } else {
            onlyNode.push(r);
        }
    }
    const onlyHeadless = [];
    for (const r of forward) {
        const k = key(r);
        if (counts.get(k) > 0) {
            counts.set(k, counts.get(k) - 1);
            onlyHeadless.push({ timestamp: r.timestamp, height: r.height, username: r.username });
        }
    }
    return {
        ok: onlyNode.length === 0 && onlyHeadless.length === 0,
        rows: corpus.reference.missed.length,
        headlessRows: forward.length,
        excluded,
        refusedWithoutHeight,
        onlyNode: onlyNode.slice(0, LIMIT),
        onlyHeadless: onlyHeadless.slice(0, LIMIT),
    };
}

/** A3 for one block: the node's columns against the headless block data. */
function blockColumns(meta, block) {
    const donations = {};
    for (const [address, value] of Object.entries(block.data.donations || {})) donations[address] = amountText(value);
    const headless = { username: block.data.username ?? null, burnedFee: amountText(block.data.burnedFee), donations };
    const nodeDonations = {};
    for (const [address, value] of Object.entries(meta.donations || {})) nodeDonations[address] = amountText(value);
    const node = { username: meta.username ?? null, burnedFee: amountText(meta.burnedFee), donations: nodeDonations };
    const sort = (o) => Object.fromEntries(Object.entries(o).sort(([a], [b]) => (a < b ? -1 : 1)));
    headless.donations = sort(headless.donations);
    node.donations = sort(node.donations);
    return sameJson(node, headless) ? null : { h: meta.h, node, headless };
}

/** The headless state projected to the API checkpoint's shape. */
function projectWallet(wallet) {
    const out = { address: wallet.getAddress(), balance: wallet.getBalance().toFixed(), nonce: wallet.getNonce().toFixed() };
    if (wallet.getPublicKey() !== undefined) out.publicKey = wallet.getPublicKey();
    if (wallet.hasAttribute("secondPublicKey")) out.secondPublicKey = wallet.getAttribute("secondPublicKey");
    if (wallet.hasAttribute("delegate.username")) {
        const d = wallet.getAttribute("delegate");
        out.delegate = {
            username: d.username,
            voteBalance: amountText(d.voteBalance),
            forgedFees: amountText(d.forgedFees),
            burnedFees: amountText(d.burnedFees),
            forgedRewards: amountText(d.forgedRewards),
            donations: amountText(d.donations),
            producedBlocks: d.producedBlocks,
            rank: d.rank,
            voters: d.voters,
            // The API writes the resignation type as its name.
            resigned: d.resigned === undefined ? undefined : d.resigned === 1 ? "permanent" : "temporary",
            lastBlock: d.lastBlock,
        };
    }
    const votingFor = {};
    for (const [name, { percent, votes }] of wallet.getVoteDistribution()) votingFor[name] = { percent, votes: amountText(votes) };
    out.votingFor = votingFor;
    return JSON.parse(JSON.stringify(out));
}

function projectCheckpointWallet(w) {
    const out = { address: w.address, balance: w.balance, nonce: w.nonce };
    if (w.publicKey !== undefined) out.publicKey = w.publicKey;
    const attributes = w.attributes || {};
    if (attributes.secondPublicKey !== undefined) out.secondPublicKey = attributes.secondPublicKey;
    if (attributes.delegate) {
        const d = attributes.delegate;
        out.delegate = {
            username: d.username,
            voteBalance: amountText(d.voteBalance),
            forgedFees: amountText(d.forgedFees),
            burnedFees: amountText(d.burnedFees),
            forgedRewards: amountText(d.forgedRewards),
            donations: amountText(d.donations),
            producedBlocks: d.producedBlocks,
            rank: d.rank,
            voters: d.voters,
            resigned: d.resigned,
            lastBlock: d.lastBlock,
        };
    }
    const votingFor = {};
    for (const [name, v] of Object.entries(w.votingFor || {})) votingFor[name] = { percent: v.percent, votes: amountText(v.votes) };
    out.votingFor = votingFor;
    return JSON.parse(JSON.stringify(out));
}

const KNOWN_ATTRIBUTES = new Set(["delegate", "secondPublicKey"]);

/** A4 for one checkpoint: the API's view against the headless state at the same height. */
function checkpointDifferences(checkpoint, chain) {
    const differences = [];
    const repository = chain.walletRepository;
    const listed = new Set();
    for (const w of checkpoint.wallets) {
        listed.add(w.address);
        for (const key of Object.keys(w.attributes || {})) {
            if (!KNOWN_ATTRIBUTES.has(key)) differences.push({ address: w.address, attribute: key, note: "an attribute no kept type sets" });
        }
        const node = projectCheckpointWallet(w);
        const headless = repository.hasByAddress(w.address) ? projectWallet(repository.findByAddress(w.address)) : null;
        if (!sameJson(node, headless)) differences.push({ address: w.address, node, headless });
    }
    for (const wallet of chain.wallets()) {
        const busy = !wallet.getBalance().isZero() || !wallet.getNonce().isZero() || wallet.getPublicKey() !== undefined || wallet.hasAttribute("delegate.username");
        if (busy && !listed.has(wallet.getAddress())) differences.push({ address: wallet.getAddress(), note: "a wallet the API did not list", headless: projectWallet(wallet) });
    }
    for (const d of checkpoint.delegates) {
        const wallet = repository.hasByUsername(d.username) ? repository.findByUsername(d.username) : null;
        if (!wallet) {
            differences.push({ delegate: d.username, note: "no headless delegate" });
            continue;
        }
        const a = wallet.getAttribute("delegate");
        const node = {
            rank: d.rank ?? null,
            votes: amountText(d.votes),
            voters: d.voters,
            isResigned: d.isResigned,
            produced: d.produced,
            lastBlock: d.lastBlock && typeof d.lastBlock === "object" ? d.lastBlock.id : (d.lastBlock ?? null),
            forged: { fees: amountText(d.forged.fees), burnedFees: amountText(d.forged.burnedFees), rewards: amountText(d.forged.rewards), donations: amountText(d.forged.donations) },
        };
        const headless = {
            rank: a.rank ?? null,
            votes: amountText(a.voteBalance),
            voters: a.voters,
            isResigned: a.resigned !== undefined,
            produced: a.producedBlocks,
            lastBlock: a.lastBlock ?? null,
            forged: { fees: amountText(a.forgedFees), burnedFees: amountText(a.burnedFees), rewards: amountText(a.forgedRewards), donations: amountText(a.donations) },
        };
        if (!sameJson(node, headless)) differences.push({ delegate: d.username, node, headless });
    }
    let supply = chain.lib.Utils.BigNumber.ZERO;
    for (const w of chain.wallets()) supply = supply.plus(w.getBalance());
    if (checkpoint.blockchain && checkpoint.blockchain.supply !== undefined && amountText(checkpoint.blockchain.supply) !== supply.toFixed()) {
        differences.push({ supply: { node: amountText(checkpoint.blockchain.supply), headless: supply.toFixed() } });
    }
    return differences;
}

// The attributes a snapshot or the API does not show: the node-local version and round marker,
// and the votes, which the API shows as votingFor.
const HIDDEN_ATTRIBUTES = [["delegate", "version"], ["delegate", "round"], ["votes"]];

/** `value` as JSON text with the keys of every object sorted. */
function stableText(value) {
    if (Array.isArray(value)) return `[${value.map(stableText).join(",")}]`;
    if (value !== null && typeof value === "object") {
        return `{${Object.keys(value)
            .sort()
            .map((k) => `${JSON.stringify(k)}:${stableText(value[k])}`)
            .join(",")}}`;
    }
    return JSON.stringify(value);
}

/**
 * A wallet's attributes as the API shows them: plain JSON, without the hidden ones, and the
 * resignation type as its name.
 */
function shownAttributes(wallet) {
    const attributes = JSON.parse(JSON.stringify(wallet.getAttributes(), (k, v) => (v instanceof Map ? [...v] : v)));
    for (const keys of HIDDEN_ATTRIBUTES) {
        const parent = keys.slice(0, -1).reduce((o, k) => (o && typeof o === "object" ? o[k] : undefined), attributes);
        if (parent && typeof parent === "object") delete parent[keys[keys.length - 1]];
    }
    if (attributes.delegate && attributes.delegate.resigned !== undefined) attributes.delegate.resigned = attributes.delegate.resigned === 1 ? "permanent" : "temporary";
    return attributes;
}

/**
 * The differences between a snapshot of the devnet tooling (height, supply, delegates, wallets:
 * the restart test's snapshots of a live node) and the headless state. The delegates' missed
 * blocks are productivity data, which the state does not hold, and are not compared.
 */
function snapshotDifferences(snapshot, chain) {
    const differences = [];
    const repository = chain.walletRepository;
    const last = chain.lastBlock().data;
    if (snapshot.height !== last.height || snapshot.blockId !== last.id) {
        differences.push({ tip: { node: [snapshot.height, snapshot.blockId], headless: [last.height, last.id] } });
    }
    const listed = new Set();
    for (const w of snapshot.wallets) {
        listed.add(w.address);
        const node = { balance: w.balance, nonce: w.nonce, votes: w.votes || {}, attributes: stableText(w.attributes || {}) };
        let headless = null;
        if (repository.hasByAddress(w.address)) {
            const wallet = repository.findByAddress(w.address);
            const votes = {};
            for (const [name, { percent, votes: amount }] of wallet.getVoteDistribution()) votes[name] = { percent, votes: amountText(amount) };
            headless = { balance: wallet.getBalance().toFixed(), nonce: wallet.getNonce().toFixed(), votes, attributes: stableText(shownAttributes(wallet)) };
        }
        if (!sameJson(node, headless)) differences.push({ address: w.address, node, headless });
    }
    for (const wallet of chain.wallets()) {
        const busy = !wallet.getBalance().isZero() || !wallet.getNonce().isZero() || wallet.getPublicKey() !== undefined || wallet.hasAttribute("delegate.username");
        if (busy && !listed.has(wallet.getAddress())) differences.push({ address: wallet.getAddress(), note: "a wallet the snapshot does not list" });
    }
    for (const d of snapshot.delegates) {
        const wallet = repository.hasByUsername(d.username) ? repository.findByUsername(d.username) : null;
        if (!wallet) {
            differences.push({ delegate: d.username, note: "no headless delegate" });
            continue;
        }
        const a = wallet.getAttribute("delegate");
        const node = { publicKey: d.publicKey, rank: d.rank ?? null, votes: amountText(d.votes), voters: d.voters, resigned: d.resigned, produced: d.produced, forged: d.forged };
        const fees = chain.lib.Utils.BigNumber.make(a.forgedFees);
        const total = fees.minus(a.burnedFees).plus(a.forgedRewards).minus(a.donations);
        const headless = {
            publicKey: wallet.getPublicKey(),
            rank: a.rank ?? null,
            votes: amountText(a.voteBalance),
            voters: a.voters,
            resigned: a.resigned !== undefined,
            produced: a.producedBlocks,
            forged: { fees: amountText(a.forgedFees), burnedFees: amountText(a.burnedFees), rewards: amountText(a.forgedRewards), donations: amountText(a.donations), total: total.toFixed() },
        };
        if (stableText(node) !== stableText(headless)) differences.push({ delegate: d.username, node, headless });
    }
    let supply = chain.lib.Utils.BigNumber.ZERO;
    for (const w of chain.wallets()) supply = supply.plus(w.getBalance());
    if (snapshot.supply !== undefined && amountText(snapshot.supply) !== supply.toFixed()) differences.push({ supply: { node: amountText(snapshot.supply), headless: supply.toFixed() } });
    return differences;
}

/** The differences between a node's state file (an API checkpoint or a snapshot) and the headless state. */
function stateDifferences(data, chain) {
    return data.kind === "checkpoint" ? checkpointDifferences(data, chain) : snapshotDifferences(data, chain);
}

// The keys a restart line shares with the continuous line of its height, and those every other
// line shares with the continuous line of the same step (the running digest cd aside: a restart
// run has more lines).
const SAME_STATE = ["b", "s", "r", "w"];
const SAME_STEP = ["h", "id", "rw", "b", "s", "r", "m", "w", "sd"];

/**
 * One restart run of `mode`: the continuous run's steps with a restart at every planned height,
 * each line compared with the continuous run's. `continuous` are the continuous lines, `forward`
 * the continuous apply line (or the genesis line) of each height. Restart lines go to `emit`.
 */
async function restartRun(context, mode) {
    const { solar, corpus, plan, steps, continuous, forward, isRoundEnd, bytesOf, live, emit } = context;
    const session = new ReplaySession(solar, corpus.crypto);
    const heights = new Set(plan.heights);
    const report = { restarts: 0, refused: 0, lines: 0, differences: [], failures: [] };
    const liveResults = [];
    let i = 0;
    const compare = (what, line, want, keys) => {
        report.lines++;
        const differing = want ? keys.filter((k) => line[k] !== want[k]) : ["the continuous run has no such line"];
        if (differing.length) report.differences.push({ what, h: line.h, step: line.step, keys: differing });
    };
    const next = (line) => {
        compare(`step ${i + 1}`, line, continuous[i], ["step", ...SAME_STEP]);
        i++;
    };
    const reapply = async (h, what) => {
        const again = await session.apply(bytesOf(h), "reapply");
        if (!again.accepted) {
            report.failures.push({ h, step: "reapply", what, error: again.error });
            return undefined;
        }
        return again.line;
    };
    const restartAt = async (h) => {
        const r = await session.restart(mode);
        report.restarts++;
        const record = r.refused ? { mode, h, step: "restart", refused: { reason: r.refused.reason } } : { mode, ...r.line };
        if (r.notes) record.notes = r.notes;
        emit(record);
        if (r.refused) {
            report.refused++;
            report.differences.push({ what: `the restart at ${h} was refused`, h, reason: r.refused.reason });
            return;
        }
        compare(`the restart at ${h}`, r.line, forward.get(h), SAME_STATE);
        for (const l of live.filter((l) => l.height === h && l.path === mode)) {
            liveResults.push({ height: h, path: mode, file: l.checkpoint, differences: stateDifferences(l.data, session.chain).slice(0, LIMIT) });
        }
        if (!isRoundEnd(h)) return;
        for (const count of plan.reverts || []) {
            if (count >= h) continue;
            for (const line of await session.revert(count)) compare(`a revert after the restart at ${h}`, line, forward.get(line.h), SAME_STATE);
            for (let k = h - count + 1; k <= h; k++) {
                const line = await reapply(k, `after the restart at ${h}`);
                if (!line) return;
                compare(`a re-apply after the restart at ${h}`, line, forward.get(k), SAME_STEP);
            }
        }
    };
    next(await session.start());
    if (heights.has(1)) await restartAt(1);
    for (const block of corpus.blocks) {
        const result = await session.apply(Buffer.from(block.hex, "hex"), "apply");
        if (!result.accepted) {
            report.failures.push({ h: block.h, step: "apply", error: result.error });
            break;
        }
        next(result.line);
        for (const count of steps.get(block.h) || []) {
            for (const line of await session.revert(count)) next(line);
            for (let h = block.h - count + 1; h <= block.h; h++) {
                const line = await reapply(h, "the revert plan");
                if (line) next(line);
            }
        }
        if (heights.has(block.h)) await restartAt(block.h);
        if (report.failures.length) break;
    }
    const last = session.lines[session.lines.length - 1];
    report.finalChainDigest = last ? last.cd : null;
    report.ok = report.refused === 0 && report.failures.length === 0 && report.differences.length === 0;
    report.differences = report.differences.slice(0, LIMIT);
    return { report, liveResults };
}

/** The restart plan in `plan`, checked against the chain's tip. */
function checkRestartPlan(plan, tip) {
    if (plan.format !== "heartwood-restart-plan/1") throw new UsageError("not a heartwood-restart-plan/1 file");
    if (plan.tip !== tip) throw new UsageError(`the restart plan was made for the tip ${plan.tip}, the chain ends at ${tip}: make it again`);
    const heights = plan && plan.heights;
    if (!Array.isArray(heights) || heights.some((h, i) => !Number.isSafeInteger(h) || h < 1 || h > tip || (i > 0 && h <= heights[i - 1]))) {
        throw new UsageError(`the restart plan's heights must rise from 1 to the tip ${tip}`);
    }
    const reverts = plan.reverts || [];
    if (!Array.isArray(reverts) || reverts.some((n) => !Number.isSafeInteger(n) || n < 1)) throw new UsageError("the restart plan's reverts must be block counts");
    const modes = (plan.modes && plan.modes.reference) || ["rebuild", "saved"];
    if (!Array.isArray(modes) || modes.some((m) => m !== "rebuild" && m !== "saved")) throw new UsageError("the restart plan's reference modes are rebuild and saved");
    return { ...plan, reverts, referenceModes: modes };
}

async function main() {
    const opts = parseArgs(process.argv.slice(2));
    assertNode18();
    const solar = loadSolar(opts.solarDir);
    let corpus = loadCorpus(path.resolve(opts.corpus));
    const milestones = [...corpus.crypto.milestones].sort((a, b) => a.height - b.height);
    const roundLength = milestones[0].activeDelegates;
    if (milestones.some((m) => m.activeDelegates !== undefined && m.activeDelegates !== roundLength)) throw new Error("the round length changes after height 1");
    const isRoundEnd = (h) => h % roundLength === 0;
    if (opts.to !== undefined) corpus = cutCorpus(corpus, opts.to, Math.floor(opts.to / roundLength) + 1);
    const plan = opts.revertPlan ? readJson(opts.revertPlan) : corpus.revertPlan;
    const givenRestartPlan = opts.restartPlan ? readJson(opts.restartPlan) : corpus.restartPlan;
    const restartPlan = givenRestartPlan ? checkRestartPlan(givenRestartPlan, corpus.blocks.length + 1) : undefined;
    const out = path.resolve(opts.out);
    const outDir = path.dirname(out);
    const anchorsFile = path.resolve(opts.anchors || path.join(outDir, "anchors.json"));
    const dumpDir = path.resolve(opts.dumpDir || outDir);
    const dumps = new Set(opts.dump);

    const session = new ReplaySession(solar, corpus.crypto);
    const write = fs.openSync(out, "w");
    const emit = (line) => fs.writeSync(write, JSON.stringify(line) + "\n");
    emit(await session.start());
    const chain = session.chain;

    const checkpoints = new Map(corpus.reference.checkpoints.map((c) => [c.data.height, c]));
    const metaByHeight = new Map(corpus.reference.meta.map((m) => [m.h, m]));
    const steps = new Map();
    for (const step of plan.steps || []) {
        if (!steps.has(step.after)) steps.set(step.after, []);
        steps.get(step.after).push(step.blocks);
    }
    const failures = [];
    const a3 = { ok: true, blocks: 0, differences: [] };
    const a4 = { ok: true, checkpoints: [] };
    let reverts = 0;
    let reapplies = 0;

    const bytesOf = (h) => Buffer.from(corpus.blocks[h - 2].hex, "hex");
    for (const block of corpus.blocks) {
        const result = await session.apply(Buffer.from(block.hex, "hex"), "apply");
        if (!result.accepted) {
            failures.push({ h: block.h, step: "apply", error: result.error });
            break;
        }
        if (result.id !== block.id) failures.push({ h: block.h, note: `the block decodes to id ${result.id}, the corpus says ${block.id}` });
        emit(result.line);
        const meta = metaByHeight.get(block.h);
        a3.blocks++;
        if (!meta) {
            a3.differences.push({ h: block.h, note: "no node columns for this block" });
        } else {
            const d = blockColumns(meta, result.block);
            if (d) a3.differences.push(d);
        }
        const checkpoint = checkpoints.get(block.h);
        if (checkpoint) {
            const differences = checkpointDifferences(checkpoint.data, chain);
            a4.checkpoints.push({ file: checkpoint.file, height: block.h, wallets: checkpoint.data.wallets.length, delegates: checkpoint.data.delegates.length, differences: differences.slice(0, LIMIT) });
        }
        if (dumps.has(block.h)) fs.writeFileSync(path.join(dumpDir, `dump-h${block.h}.json`), JSON.stringify({ h: block.h, step: "apply", sections: session.sections() }, null, 1) + "\n");
        for (const count of steps.get(block.h) || []) {
            for (const line of await session.revert(count)) emit(line);
            reverts += count;
            for (let h = block.h - count + 1; h <= block.h; h++) {
                const again = await session.apply(bytesOf(h), "reapply");
                if (!again.accepted) {
                    failures.push({ h, step: "reapply", error: again.error });
                    break;
                }
                emit(again.line);
                reapplies++;
            }
        }
        if (failures.length) break;
    }
    fs.closeSync(write);

    for (const c of corpus.reference.checkpoints) {
        if (!checkpoints.has(c.data.height) || c.data.height === 1 || c.data.height > corpus.blocks.length + 1) {
            a4.checkpoints.push({ file: c.file, height: c.data.height, differences: [{ note: "no block of the corpus at this height" }] });
        }
    }
    a3.ok = a3.differences.length === 0;
    a3.differences = a3.differences.slice(0, LIMIT);
    // A chain cut below its checkpoints keeps A1 to A3 and A5 as its anchors.
    if (corpus.cut !== undefined && a4.checkpoints.length === 0) a4.note = `no checkpoint at or below the cut ${corpus.cut}`;
    a4.ok = (a4.checkpoints.length > 0 || corpus.cut !== undefined) && a4.checkpoints.every((c) => c.differences.length === 0);
    const archives = (corpus.meta.archives || []).map((a) => ({ from: a.from, to: a.to, verified: a.verified === true }));
    const a5 = { ok: archives.length > 0 && archives.every((a) => a.verified), archives };
    const anchors = {
        A1: anchorRounds(corpus, chain.store),
        A2: anchorMissed(corpus, chain.store),
        A3: a3,
        A4: a4,
        A5: a5,
    };
    let restarts;
    if (restartPlan) {
        const restartsFile = path.resolve(opts.restarts || path.join(outDir, "ts-restarts.jsonl"));
        const writeRestarts = fs.openSync(restartsFile, "w");
        const forward = new Map();
        for (const line of session.lines) if (line.step === "genesis" || line.step === "apply") forward.set(line.h, line);
        const context = {
            solar,
            corpus,
            plan: restartPlan,
            steps,
            continuous: session.lines,
            forward,
            isRoundEnd,
            bytesOf,
            live: corpus.reference.restarts,
            emit: (record) => fs.writeSync(writeRestarts, JSON.stringify(record) + "\n"),
        };
        const modes = {};
        const liveResults = [];
        for (const mode of restartPlan.referenceModes) {
            const run = await restartRun(context, mode);
            modes[mode] = run.report;
            liveResults.push(...run.liveResults);
        }
        fs.closeSync(writeRestarts);
        restarts = {
            ok: Object.values(modes).every((m) => m.ok),
            plan: { kind: restartPlan.kind ?? null, seed: restartPlan.seed ?? null, heights: restartPlan.heights.length, reverts: restartPlan.reverts },
            modes,
        };
        if (corpus.reference.restarts.length) {
            const missing = corpus.reference.restarts
                .filter((l) => !liveResults.some((r) => r.height === l.height && r.path === l.path))
                .map((l) => ({ height: l.height, path: l.path, file: l.checkpoint, differences: [{ note: "the restart plan does not restart this way at this height" }] }));
            const all = [...liveResults, ...missing].sort((a, b) => a.height - b.height || a.path.localeCompare(b.path));
            anchors.A6 = { ok: all.every((r) => r.differences.length === 0), restarts: all };
        }
    }
    let processor;
    if (opts.processor) {
        const { processorJudge } = require("./lib/ops/job");
        processor = await processorJudge({ solar, corpus, out: path.join(outDir, "ts-processor.jsonl") });
    }
    const last = session.lines[session.lines.length - 1];
    const ok = failures.length === 0 && Object.values(anchors).every((a) => a.ok) && (!processor || processor.ok);
    const report = {
        format: "heartwood-judge/1",
        ok,
        reference: { version: solar.version, commit: solar.provenance.commit, tree: solar.provenance.tree, seriesSha256: solar.seriesSha256 },
        oracle: { mode: process.env.ORACLE_IMAGE_ID ? "container" : "local", image: process.env.ORACLE_IMAGE_ID || null, node: process.version },
        blocks: corpus.blocks.length,
        tip: chain.lastBlock().data.height,
        lines: session.lines.length,
        reverts,
        reapplies,
        finalChainDigest: last ? last.cd : null,
        failures,
        anchors,
    };
    if (opts.to !== undefined) report.to = opts.to;
    if (restarts) report.restarts = restarts;
    if (processor) report.processor = processor;
    fs.writeFileSync(anchorsFile, JSON.stringify(report, null, 1) + "\n");
    const summary = Object.entries(anchors)
        .map(([k, a]) => `${k} ${a.ok ? "ok" : "FAIL"}`)
        .join(", ");
    console.log(`replay-chain: ${corpus.blocks.length} blocks, ${session.lines.length} lines (${reverts} reverts, ${reapplies} re-applies); ${summary}; ${ok ? "judge ok" : "NOT a judge"}`);
    if (restarts) {
        for (const [mode, m] of Object.entries(restarts.modes)) {
            console.log(`replay-chain: restarts by ${mode}: ${m.restarts} (${m.refused} refused), ${m.lines} lines compared, ${m.differences.length} differences${m.failures.length ? `, ${m.failures.length} failures` : ""}`);
        }
    }
    return ok ? 0 : 1;
}

main().then(
    (code) => process.exit(code),
    (error) => {
        console.error(`replay-chain: ${error instanceof UsageError ? error.message : error.stack}`);
        process.exit(error instanceof UsageError ? 2 : 1);
    },
);
