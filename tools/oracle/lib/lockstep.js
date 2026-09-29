"use strict";
// The lockstep session: the reference's side of a chain the producer forges with Heartwood, a
// replay session (lib/corpus.js over lib/state.js) with a reference pool (lib/pool.js) bound in
// its node, each block applied with its digest line and each transaction built once and
// submitted to the pool. It is a chain session of the registry the pool operations share, so
// pool.contents, pool.collate and the other pool operations take its id too. Used by
// lib/ops/lockstep.js.
//
// The producer resolves labels to keys: a build spec here carries the sender's passphrase, the
// nonce, and the type's own fields with addresses in place of labels.

const path = require("path");

const { ReplaySession } = require("./corpus");
const { HeadlessPool, sessions: registry } = require("./pool");
const { newDataDir } = require("./state");

const BUILDERS = new Set(["transfer", "secondSignature", "vote", "delegateRegistration", "delegateResignation", "burn", "ipfs"]);

// A fee to build with before the floor is known: its bytes are as long as any fee's.
const PLACEHOLDER_FEE = "10000000000";

/** The operations of the lockstep session, over the helpers of lib/ops.js (its `context`). */
function lockstepOps(context) {
    const { solar, devnet, safeInteger, hexBuffer, cm, handlerRegistry } = context;
    const { Transactions } = solar.lib;

    const session = (id) => {
        const s = registry.get(id);
        if (s.kind !== "chain" || !s.replay.chain.pool) throw new Error(`session ${id} is not a lockstep session`);
        return s.replay;
    };

    // Run fn under the session's configuration, then restore the devnet configuration.
    const inSession = (s, fn) => s.chain.withConfig(() => devnet.restore(), fn);

    function text(value, name) {
        if (typeof value !== "string") throw new TypeError(`${name} must be text`);
        return value;
    }

    /** One builder with the calls of a spec, signed and built; `fee` overrides the spec's. */
    function make(spec, fee) {
        if (spec === null || typeof spec !== "object") throw new TypeError("a build spec must be an object");
        if (!BUILDERS.has(spec.kind)) throw new TypeError(`unknown kind ${spec.kind}`);
        const asset = spec.asset || {};
        let b = Transactions.BuilderFactory[spec.kind]();
        switch (spec.kind) {
            case "transfer": {
                if (!Array.isArray(asset.recipients) || asset.recipients.length === 0) throw new TypeError("a transfer needs recipients");
                for (const [address, value] of asset.recipients) b = b.addTransfer(text(address, "address"), text(value, "amount"));
                break;
            }
            case "secondSignature":
                b = b.signatureAsset(text(asset.secondPassphrase, "secondPassphrase"));
                break;
            case "vote": {
                const votes = {};
                for (const [name, percent] of asset.votes || []) votes[text(name, "name")] = Number(percent);
                b = b.votesAsset(votes);
                break;
            }
            case "delegateRegistration":
                b = b.usernameAsset(text(asset.username, "username"));
                break;
            case "delegateResignation":
                b = b.resignationTypeAsset(Number(asset.resignationType ?? 0));
                break;
            case "burn":
                b = b.amount(text(asset.amount, "amount"));
                break;
            case "ipfs":
                b = b.ipfsAsset(text(asset.hash, "hash"));
                break;
        }
        b = b.nonce(text(spec.nonce, "nonce")).fee(fee);
        if (spec.memo !== undefined && spec.memo !== null) b = b.memo(text(spec.memo, "memo"));
        return b.sign(text(spec.passphrase, "passphrase")).build();
    }

    /** The transaction of a spec, at the network's floor fee when the spec has none. */
    function build(spec, height) {
        if (spec.fee !== undefined && spec.fee !== null) return make(spec, text(spec.fee, "fee"));
        const draft = make(spec, PLACEHOLDER_FEE);
        const { InternalTransactionType } = Transactions;
        const handler = handlerRegistry().getRegisteredHandlerByType(InternalTransactionType.from(draft.data.type, draft.data.typeGroup));
        const floor = handler.getMinimumFee(draft, cm.getMilestone(height).dynamicFees);
        return make(spec, floor.toString());
    }

    return {
        // {crypto: {network, milestones, genesisBlock, exceptions}, options?} -> {session, line}
        "lockstep.open": async ({ crypto, options }) => {
            if (!crypto || typeof crypto !== "object") throw new TypeError("lockstep.open needs the chain's crypto files");
            const bindPool = HeadlessPool.binding(solar, options || {}, path.join(newDataDir("pool"), "pool.sqlite"));
            const s = new ReplaySession(solar, JSON.parse(JSON.stringify(crypto)), { bindPool });
            const line = await inSession(s, () => s.start());
            return { session: registry.add({ kind: "chain", replay: s }), line };
        },

        // {session, build: [spec]} -> {transactions: [{id, hex}]}, at the height of the call
        "lockstep.build": async ({ session: id, build: specs }, height) => {
            if (!Array.isArray(specs)) throw new TypeError("build must be a list of specs");
            const s = session(id);
            return inSession(s, async () => ({
                transactions: specs.map((spec) => {
                    const tx = build(spec, height);
                    return { id: tx.id, hex: tx.serialised.toString("hex") };
                }),
            }));
        },

        // {session, transactions: [hex], now} -> the pool processor's result on the api channel
        "lockstep.submit": async ({ session: id, transactions, now }) => {
            if (!Array.isArray(transactions)) throw new TypeError("transactions must be a list of hex");
            safeInteger(now, "now");
            const s = session(id);
            return inSession(s, () => s.chain.pool.process(transactions.map((hex) => ({ hex: hexBuffer(hex, "transaction").toString("hex") })), now));
        },

        // {session, round, delegates: [publicKey], expireAt?} -> {online}
        "lockstep.online": async ({ session: id, round, delegates, expireAt }) => {
            safeInteger(round, "round");
            if (!Array.isArray(delegates)) throw new TypeError("delegates must be a list of public keys");
            const s = session(id);
            return inSession(s, () => ({ online: s.chain.pool.setOnline(delegates.map((publicKey) => ({ publicKey, round })), expireAt) }));
        },

        // {session, hex} -> the block as a synced node applies it: {accepted, height, id, reward?,
        // missed, line?, stage?, error?}, its transactions then removed from the pool, the pool
        // cleaned up and drained
        "lockstep.apply": async ({ session: id, hex, step = "apply", expireAt }) => {
            if (step !== "apply" && step !== "reapply") throw new TypeError("step must be apply or reapply");
            const s = session(id);
            const bytes = hexBuffer(hex, "hex");
            return inSession(s, async () => {
                const pool = s.chain.pool;
                const r = await s.apply(bytes, step);
                const out = { accepted: r.accepted, height: r.height, id: r.id, missed: r.missed };
                if (r.reward !== undefined) out.reward = r.reward;
                if (!r.accepted) return { ...out, stage: r.stage, error: { class: r.error.class, message: r.error.message.split(solar.solarDir).join("<reference>") } };
                if (expireAt !== undefined) pool.setOnline([], safeInteger(expireAt, "expireAt"));
                for (const transaction of r.block.transactions) await pool.service.removeForgedTransaction(transaction);
                await pool.cleanUp();
                await pool.readd([], true);
                return { ...out, line: r.line };
            });
        },

        // {session, count} -> {lines}: `count` blocks reverted as the blockchain's removeBlocks does,
        // then their transactions offered to the pool again, in chain order
        "lockstep.revert": async ({ session: id, count }) => {
            safeInteger(count, "count");
            const s = session(id);
            return inSession(s, async () => {
                const store = s.chain.store;
                const tip = store.height;
                const forged = [];
                for (let h = tip - count + 1; h <= tip; h++) {
                    const entry = store.blocks[h - 1];
                    if (!entry) throw new RangeError(`no stored block ${h}`);
                    for (const t of entry.transactions) forged.push(t.serialised.toString("hex"));
                }
                const lines = await s.revert(count);
                await s.chain.pool.readd(forged, false);
                return { lines };
            });
        },

        "lockstep.close": async ({ session: id }) => {
            const s = session(id);
            s.chain.pool.dispose();
            registry.delete(id);
            return { closed: true };
        },
    };
}

module.exports = { lockstepOps };
