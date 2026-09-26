#!/usr/bin/env node
"use strict";
// replay-chain.js: the L2 judge. It replays a corpus chain on the reference implementation's own
// state machine, run headless in this process (lib/state.js), follows the corpus's revert plan,
// writes one state digest line per step (lib/digest.js), and checks that the headless replay is
// the live node's chain (the anchors).
//
//   node replay-chain.js --corpus DIR --out FILE [--anchors FILE] [--revert-plan FILE]
//                        [--dump H,...] [--dump-dir DIR] [--solar-dir DIR]
//
// --out        the digest lines, one JSON object per step (ts-digests.jsonl)
// --anchors    the anchor results [anchors.json next to --out]
// --revert-plan  a revert plan instead of the corpus's revert-plan.json
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
// Exit status: 0 when every block applied and every anchor holds, 1 otherwise, 2 on a usage error.

const fs = require("fs");
const path = require("path");

const { assertNode18, loadSolar } = require("./lib/solar");
const { loadCorpus, readJson, ReplaySession } = require("./lib/corpus");

function parseArgs(argv) {
    const opts = { corpus: undefined, out: undefined, anchors: undefined, revertPlan: undefined, dump: [], dumpDir: undefined, solarDir: undefined };
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
        else if (a === "--dump") opts.dump = value().split(",").filter(Boolean).map(Number);
        else if (a === "--dump-dir") opts.dumpDir = value();
        else if (a === "--solar-dir") opts.solarDir = value();
        else throw new UsageError(`unknown argument ${a}`);
    }
    if (!opts.corpus || !opts.out) throw new UsageError("usage: replay-chain.js --corpus DIR --out FILE [--anchors FILE] [--revert-plan FILE] [--dump H,...] [--dump-dir DIR] [--solar-dir DIR]");
    if (opts.dump.some((h) => !Number.isSafeInteger(h) || h < 1)) throw new UsageError("--dump takes heights");
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
    const forward = store.missed.filter((r) => r.step === "start" || (r.step && r.step.startsWith("apply:")));
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

async function main() {
    const opts = parseArgs(process.argv.slice(2));
    assertNode18();
    const solar = loadSolar(opts.solarDir);
    const corpus = loadCorpus(path.resolve(opts.corpus));
    const plan = opts.revertPlan ? readJson(opts.revertPlan) : corpus.revertPlan;
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
    a4.ok = a4.checkpoints.length > 0 && a4.checkpoints.every((c) => c.differences.length === 0);
    const archives = (corpus.meta.archives || []).map((a) => ({ from: a.from, to: a.to, verified: a.verified === true }));
    const a5 = { ok: archives.length > 0 && archives.every((a) => a.verified), archives };
    const anchors = {
        A1: anchorRounds(corpus, chain.store),
        A2: anchorMissed(corpus, chain.store),
        A3: a3,
        A4: a4,
        A5: a5,
    };
    const last = session.lines[session.lines.length - 1];
    const ok = failures.length === 0 && Object.values(anchors).every((a) => a.ok);
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
    fs.writeFileSync(anchorsFile, JSON.stringify(report, null, 1) + "\n");
    const summary = Object.entries(anchors)
        .map(([k, a]) => `${k} ${a.ok ? "ok" : "FAIL"}`)
        .join(", ");
    console.log(`replay-chain: ${corpus.blocks.length} blocks, ${session.lines.length} lines (${reverts} reverts, ${reapplies} re-applies); ${summary}; ${ok ? "judge ok" : "NOT a judge"}`);
    return ok ? 0 : 1;
}

main().then(
    (code) => process.exit(code),
    (error) => {
        console.error(`replay-chain: ${error instanceof UsageError ? error.message : error.stack}`);
        process.exit(error instanceof UsageError ? 2 : 1);
    },
);
