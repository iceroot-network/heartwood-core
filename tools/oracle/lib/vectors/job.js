"use strict";
// V27, the job (chain.job, lib/ops/job.js):
//   - the enqueueing's cuts: 150 transactions, 100 blocks, milestone heights with the list that
//     shifts only on a cut (a milestone at exactly the last downloaded height suppresses the later
//     cuts), the empty trailing job, and no block at all;
//   - jobs on a chain of the devnet generator's first chain definition, forged here by the
//     scheduled delegates up to height 50 and then run as one session: an empty job, a job at or
//     below the tip, an unchained first block, a future slot, a job across a round end, a block
//     seen twice, a rebuild that fails as the first block and after an accepted one, a
//     double-forged block, and each unchained status inside a job;
//   - two jobs whose Heartwood side is a documented difference, each in a record of its own: a
//     block refused in its apply after missed slots (the reference keeps the rows it recorded
//     before the refusal) and a block with a transaction of a removed type (the reference refuses
//     it when it applies the block, Heartwood's download refuses it).
// The chain needs the devnet generator's chain definition (V10); without the generator only the
// enqueueing's records are written.
//
// Blocks and transactions are signed with the oracle's fixed aux, so the records are the same on
// every run.

const { ReplaySession } = require("../corpus");
const { FIXED_AUX } = require("../ops");

const CRYPTO_FILES = ["network", "milestones", "genesisBlock", "exceptions"];
// The chain's tip before its jobs.
const PREFIX = 50;
// A fee above every fee floor of the devnet for a small transaction.
const FEE = "20000000";
// A header field a listing states otherwise than the block: the rebuild refuses the block.
const WRONG_COUNT = (hex, count) => ({ hex, set: { numberOfTransactions: count + 1 } });

/** The enqueueing's records. */
async function enqueueRecords({ devnet, rec, must }) {
    const out = [];
    const milestones = (heights) => JSON.stringify([...devnet.config.milestones, ...heights.map((height) => ({ height, block: { ...devnet.config.milestones[0].block } }))]);
    const blocks = (from, counts) => counts.map((numberOfTransactions, i) => ({ height: from + i, numberOfTransactions }));
    const empty = (from, n) => blocks(from, new Array(n).fill(0));
    const cut = async (name, enqueue, heights) => {
        const input = { enqueue };
        if (heights) input.milestones = milestones(heights);
        out.push({ ...must(await rec("chain.job", 1, input)), name: `enqueue: ${name}` });
    };
    await cut("150 transactions close a job", { blocks: blocks(11, [100, 49, 1, 5]), lastDownloadedHeight: 10 });
    await cut("149 transactions do not", { blocks: blocks(11, [100, 49]), lastDownloadedHeight: 10 });
    await cut("a block of 150 transactions is a job of its own", { blocks: blocks(11, [0, 150, 0]), lastDownloadedHeight: 10 });
    await cut("100 blocks close a job, and the empty trailing job is pushed", { blocks: empty(11, 100), lastDownloadedHeight: 10 });
    await cut("250 blocks", { blocks: empty(2, 250), lastDownloadedHeight: 1 });
    await cut("a larger maxLastBlocks still closes at 100", { blocks: empty(11, 120), lastDownloadedHeight: 10, maxLastBlocks: 150 });
    await cut("a milestone closes a job at its height", { blocks: empty(11, 5), lastDownloadedHeight: 10 }, [12]);
    await cut("a milestone at the first block", { blocks: empty(11, 3), lastDownloadedHeight: 10 }, [11]);
    await cut("milestones close jobs one after another as the list shifts", { blocks: empty(11, 6), lastDownloadedHeight: 10 }, [12, 14]);
    await cut("a milestone at the last downloaded height suppresses the later cuts", { blocks: empty(11, 6), lastDownloadedHeight: 10 }, [10, 13]);
    await cut("a milestone below the last downloaded height is left out", { blocks: empty(11, 4), lastDownloadedHeight: 10 }, [8, 12]);
    await cut("a job closed by transactions before a milestone", { blocks: blocks(11, [150, 0, 0, 0]), lastDownloadedHeight: 10 }, [13]);
    await cut("the last block at a milestone leaves an empty trailing job", { blocks: empty(11, 2), lastDownloadedHeight: 10 }, [12]);
    await cut("the devnet's own milestone at height 2, last downloaded at 1", { blocks: empty(2, 3), lastDownloadedHeight: 1 });
    await cut("no block", { blocks: [], lastDownloadedHeight: 10 });
    return out;
}

/**
 * A forger on the session `s`: the next block by the delegate scheduled for its slot, in the first
 * slot at least `gap` slots after the next one, with `transactions` (built transactions), signed
 * by `signer` (a passphrase; the scheduled delegate's when absent). Returns the block's hex and
 * data; the session does not apply it.
 */
function forger(solar, s, passphrases) {
    const { Blocks, Crypto, Identities, Utils } = solar.lib;
    const chain = s.chain;
    const { Utils: AppUtils, Container } = chain.pkgs.kernel;
    const triggers = chain.app.get(Container.Identifiers.TriggerService);
    return async function forge({ gap = 0, transactions = [], signer } = {}) {
        const last = chain.lastBlock().data;
        const height = last.height + 1;
        const blockTime = solar.lib.Managers.configManager.getMilestone(height).blockTime;
        const lookup = await AppUtils.forgingInfoCalculator.getBlockTimeLookup(chain.app, height);
        const delegates = await triggers.call("getActiveDelegates", { roundInfo: AppUtils.roundCalculator.calculateRound(height) });
        for (let extra = gap; extra < gap + 200; extra++) {
            const timestamp = last.timestamp + blockTime * (1 + extra);
            const info = AppUtils.forgingInfoCalculator.calculateForgingInfo(timestamp, height, lookup);
            const scheduled = delegates[info.currentForger];
            const username = scheduled && scheduled.getAttribute("delegate.username");
            if (!username || !passphrases[username]) continue;
            const { reward } = await chain.roundState.getRewardForBlockInRound(height, scheduled);
            let amount = Utils.BigNumber.ZERO;
            let fee = Utils.BigNumber.ZERO;
            for (const t of transactions) {
                amount = amount.plus(t.data.amount || Utils.BigNumber.ZERO);
                fee = fee.plus(t.data.fee);
            }
            const keys = Identities.Keys.fromPassphrase(signer || passphrases[username]);
            const block = Blocks.BlockFactory.make(
                {
                    version: 0,
                    timestamp,
                    height,
                    previousBlock: last.id,
                    numberOfTransactions: transactions.length,
                    totalAmount: amount,
                    totalFee: fee,
                    reward,
                    payloadLength: 32 * transactions.length,
                    payloadHash: Crypto.HashAlgorithms.sha256(transactions.map((t) => Buffer.from(t.id, "hex"))).toString("hex"),
                    transactions: transactions.map((t) => t.data),
                },
                keys,
                FIXED_AUX,
            );
            return { hex: block.serialised, data: block.data, scheduled: username };
        }
        throw new Error(`no slot at height ${height} has a delegate of the chain definition`);
    };
}

/** The session's slot time of a block's timestamp, in milliseconds since the Unix epoch. */
const slotTime = (epochMs, block) => epochMs + block.data.timestamp * 1000;

/** The chain, the blocks of its jobs and the jobs, forged on the first chain definition. */
async function scenarios({ solar, devnet, shared, craft }) {
    const definition = shared.genesisDefinitions[0];
    const crypto = Object.fromEntries(CRYPTO_FILES.map((f) => [f, JSON.parse(definition.files[`crypto/${f}.json`])]));
    const passphrases = definition.passphrases;
    const { Identities, Transactions } = solar.lib;
    const epochMs = new Date(crypto.milestones[0].epoch).getTime();
    const b = new ReplaySession(solar, crypto);
    const made = {};
    await b.chain.withConfig(
        () => devnet.restore(),
        async () => {
            await b.start();
            const forge = forger(solar, b, passphrases);
            const network = crypto.network.pubKeyHash;
            const address = (role) => Identities.Address.fromPassphrase(passphrases[role], network);
            const nonce = (role) => b.chain.walletRepository.getNonce(address(role)).plus(1).toFixed();
            const transfer = (role, to, amount, at = nonce(role)) =>
                Transactions.BuilderFactory.transfer().addTransfer(address(to), amount).nonce(at).fee(FEE).sign(passphrases[role]).build();
            const apply = async (block) => {
                const r = await b.apply(Buffer.from(block.hex, "hex"));
                if (!r.accepted) throw new Error(`the builder's block ${block.data.height} was refused: ${r.error.class}: ${r.error.message}`);
                return block;
            };
            const chain = [];
            while (b.chain.lastBlock().data.height < PREFIX) {
                const height = b.chain.lastBlock().data.height + 1;
                let transactions = [];
                // wallet/1 is funded, then sends once, so that its key is bound before the jobs.
                if (height === 10) transactions = [transfer("genesis-1", "wallet/1", "5000000000")];
                if (height === 20) transactions = [transfer("wallet/1", "wallet/2", "100000000")];
                chain.push(await apply(await forge({ transactions })));
            }
            made.chain = chain;
            // Ahead of the chain: the blocks of the jobs, each forged on its parent.
            const ahead = async (options) => apply(await forge(options));
            made.b51 = await ahead();
            made.b52 = await ahead();
            made.b53 = await ahead();
            made.b54 = await ahead();
            made.b55 = await ahead();
            made.b56 = await ahead();
            made.b57x = await forge({ transactions: [transfer("wallet/1", "wallet/4", "100000000")] });
            made.b57 = await ahead();
            made.b58 = await ahead();
            // b59 leaves a slot free before it, which y59 takes: an earlier timestamp at its height.
            made.y59 = await forge();
            made.b59 = await ahead({ gap: 1 });
            made.b60 = await ahead();
            // z60: at height 60 on b59, in a slot after b60's, signed by a registered delegate that
            // is not the one scheduled there.
            await b.revert(1);
            const scheduled = (await forge({ gap: 3 })).scheduled;
            const other = Object.keys(passphrases).find((name) => name.startsWith("genesis_") && name !== scheduled);
            made.z60 = await forge({ gap: 3, signer: passphrases[other] });
            await apply(made.b60);
            made.b61 = await ahead();
            made.b62 = await ahead();
            made.b63 = await ahead();
            made.b64 = await ahead({ transactions: [transfer("wallet/1", "wallet/3", "100000000")] });
            // A signature that does not verify: one byte of b62's signature changed.
            const signatureAt = 2 * (4 + 4 + 4 + 32 + 4 + 8 + 8 + 8 + 4 + 32 + 33);
            const flipped = (made.b62.hex[signatureAt + 20] === "0" ? "1" : "0");
            made.badSignature62 = { hex: made.b62.hex.slice(0, signatureAt + 20) + flipped + made.b62.hex.slice(signatureAt + 21), data: made.b62.data };
            // The documented jobs, on the chain's tip.
            await b.revert(b.chain.lastBlock().data.height - PREFIX);
            const overspend = [transfer("wallet/1", "wallet/5", "3000000000"), transfer("wallet/1", "wallet/6", "3000000000", b.chain.walletRepository.getNonce(address("wallet/1")).plus(2).toFixed())];
            made.refusedAfterGap = await forge({ gap: 3, transactions: overspend });
            const ipfsData = craft.builderData("transfer", [["nonce", nonce("wallet/1")]], passphrases["wallet/1"]);
            Object.assign(ipfsData, { type: 5, asset: { ipfs: "QmR45FmbVVrixReBwJkhEKde2qwHYaQzGxu4ZoDeswuF9w" } });
            const ipfsHex = craft.signAndSerialise(ipfsData, passphrases["wallet/1"]).toString("hex");
            const ipfs = Transactions.TransactionFactory.fromBytesUnsafe(Buffer.from(ipfsHex, "hex"));
            made.removedType = await forge({ transactions: [ipfs] });
        },
    );
    const at = (block) => slotTime(epochMs, block);
    const jobs = [
        { name: "no block", job: { hexes: [], now: at(made.b51) } },
        { name: "a block below the tip", job: { hexes: [made.chain[45 - 2].hex], now: at(made.b51), downloaded: false } },
        { name: "a first block that does not chain", job: { hexes: [made.b52.hex], now: at(made.b52) } },
        { name: "a block in a future slot after an accepted one", job: { hexes: [made.b51.hex, made.b52.hex], now: at(made.b51) } },
        { name: "three blocks across a round end, while syncing", job: { hexes: [made.b52.hex, made.b53.hex, made.b54.hex], now: at(made.b54), machineState: "syncing" } },
        { name: "the tip again", job: { hexes: [made.b54.hex], now: at(made.b55) } },
        { name: "a block and the same block again", job: { hexes: [made.b55.hex, made.b55.hex], now: at(made.b55) } },
        { name: "a rebuild that fails as the first block", job: { hexes: [WRONG_COUNT(made.b56.hex, 0)], now: at(made.b56) } },
        { name: "a rebuild that fails after an accepted block", job: { hexes: [made.b56.hex, WRONG_COUNT(made.b57.hex, 0)], now: at(made.b57) } },
        { name: "a double-forged block after an accepted one", job: { hexes: [made.b57.hex, made.b57x.hex], now: at(made.b57) } },
        { name: "a block not ready after an accepted one", job: { hexes: [made.b58.hex, made.b60.hex], now: at(made.b60) } },
        { name: "an earlier timestamp at the tip's height", job: { hexes: [made.b59.hex, made.y59.hex], now: at(made.b59) } },
        { name: "a generator that is not the slot's at the tip's height", job: { hexes: [made.b60.hex, made.z60.hex], now: at(made.z60) } },
        { name: "a block already in the chain after an accepted one", job: { hexes: [made.b61.hex, made.b59.hex], now: at(made.b61) } },
        { name: "a block whose signature does not verify", job: { hexes: [made.badSignature62.hex], now: at(made.b62) } },
        { name: "two blocks before the blockchain started", job: { hexes: [made.b62.hex, made.b63.hex], now: at(made.b63), started: false } },
        { name: "a block with a transaction and the same block again", job: { hexes: [made.b64.hex, made.b64.hex], now: at(made.b64) } },
    ];
    const chainInput = { crypto, blocks: made.chain.map((block) => block.hex) };
    const documented = [
        { name: "a block refused in its apply after missed slots", job: { hexes: [made.refusedAfterGap.hex], now: at(made.refusedAfterGap) } },
        { name: "a block with a transaction of a removed type", job: { hexes: [made.removedType.hex], now: at(made.removedType) } },
    ];
    return { chainInput, jobs, documented };
}

module.exports = async function jobVectors(ctx) {
    const { rec, must, shared } = ctx;
    const out = await enqueueRecords(ctx);
    if (!shared.genesisDefinitions) return out;
    const { chainInput, jobs, documented } = await scenarios(ctx);
    const r = must(await rec("chain.job", 1, { chain: chainInput, jobs: jobs.map((j) => j.job) }));
    out.push({ ...r, name: `jobs: ${jobs.map((j) => j.name).join("; ")}` });
    for (const { name, job } of documented) {
        out.push({ ...must(await rec("chain.job", 1, { chain: chainInput, jobs: [job] })), name: `documented: ${name}` });
    }
    return out;
};
