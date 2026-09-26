"use strict";
// The canonical state digest (format csd1) of the reference's state: the committed state at a
// height as five text sections and their hashes. heartwood-state computes the same digest of its
// own state; both follow this definition.
//
// A section is its tag line and its records, every line ending in "\n". A record is one JSON
// value without whitespace; an object's keys are in the order below, and a key is left out when
// its value is absent. Amounts, balances, nonces and vote balances are decimal strings; heights,
// rounds, ranks, counts, timestamps and basis points are integers; hex is lowercase.
//
//   #B  one record, the last block: h, id, ts, gen (generator key), reward, fee (total fee),
//       burned (burned fee), don ([[address, amount], ...]: every donation of the block, zero
//       amounts included, sorted by the address text; [] when none).
//   #S  one record: issued (the genesis issuance), supply (the sum of all balances).
//   #R  one record: round and rh (the round of the last block's height h and its first height),
//       order (the public keys of the forging list that schedules block h + 1, in order), prod
//       (the usernames of the round's blocks after the genesis block, in order), and, only when
//       h is 1 or block h ends a round, active ([[publicKey, voteBalance], ...]: the saved rows of
//       the round of h + 1, in rank order).
//   #M  one record per slot missed before the block of this step: [timestamp, username], in slot
//       order; none for a revert.
//   #W  one record per wallet with a balance, a nonce, a public key, a second key, a delegate
//       record or votes, sorted by the address text: a, pk, bal, n, spk, then for a delegate u,
//       vb (vote balance), vc (voters), rk (rank, when ranked), rs (0 temporary, 1 permanent,
//       when resigned), pb (produced blocks), ff (forged fees), fr (forged rewards), bf (burned
//       fees), dn (donations), lb (last block id, when set), then v ([[username, basisPoints],
//       ...] in the order the votes are kept, when there are votes). The delegate's version and
//       round marker are not part of it; a wallet with a multi-signature, HTLC lock or IPFS
//       attribute is an error.
//
// Hashes (SHA-256 with the registry's domain tags; H(tag, x) = SHA-256(u8(len(tag)) || tag || x)):
//   S_x = H("heartwood/csd1/section", section)            for x in B, S, R, M, W
//   SD  = H("heartwood/csd1/height", S_B || S_S || S_R || S_M || S_W)
//   CD  = H("heartwood/csd1/chain", CD_previous || SD), from 32 zero bytes before a run's first line
//
// A line of a replay: {h, step, id, rw, b, s, r, m, w, sd, cd}; step is genesis, apply, revert
// or reapply; rw (a decimal string) is the reward the state computed for the block before
// applying it, absent for the genesis step and reverts; b to w are the section hashes.
//
// The digest of a wallet fixture (state.digest) has no block and no round: #B, #R and #M are
// their tag lines alone, and #S holds issued "0" with the supply of the fixture's wallets.

const crypto = require("crypto");

const TAGS = {
    section: "heartwood/csd1/section",
    height: "heartwood/csd1/height",
    chain: "heartwood/csd1/chain",
};
const ZERO = Buffer.alloc(32);

function taggedHash(tag, ...parts) {
    const t = Buffer.from(tag, "ascii");
    const h = crypto.createHash("sha256").update(Buffer.from([t.length])).update(t);
    for (const p of parts) h.update(p);
    return h.digest();
}

function section(tag, records) {
    return `#${tag}\n` + records.map((r) => `${JSON.stringify(r)}\n`).join("");
}

function byText(a, b) {
    return a < b ? -1 : a > b ? 1 : 0;
}

function amountText(value) {
    if (value === undefined || value === null) return "0";
    return typeof value === "string" ? value : value.toFixed();
}

/** #B of a block (the reference's IBlock). */
function blockSection(block) {
    const d = block.data;
    const don = Object.entries(d.donations || {})
        .map(([address, value]) => [address, amountText(value)])
        .sort((x, y) => byText(x[0], y[0]));
    return section("B", [
        {
            h: d.height,
            id: d.id,
            ts: d.timestamp,
            gen: d.generatorPublicKey,
            reward: amountText(d.reward),
            fee: amountText(d.totalFee),
            burned: amountText(d.burnedFee),
            don,
        },
    ]);
}

function supplySection(issued, wallets, BigNumber) {
    let supply = BigNumber.ZERO;
    for (const w of wallets) supply = supply.plus(w.getBalance());
    return section("S", [{ issued: amountText(issued), supply: supply.toFixed() }]);
}

/**
 * #R. `forging` is the forging list (wallets), `blocks` the round's blocks, `activeRows` the saved
 * round rows of round(h + 1) when h is 1 or ends a round, else undefined.
 */
function roundSection(roundInfo, forging, blocks, activeRows) {
    const record = {
        round: roundInfo.round,
        rh: roundInfo.roundHeight,
        order: forging.map((w) => w.getPublicKey()),
        prod: blocks.filter((b) => b.data.height > 1).map((b) => b.data.username),
    };
    if (activeRows !== undefined) record.active = activeRows.map((r) => [r.publicKey, amountText(r.balance)]);
    return section("R", [record]);
}

function missedSection(rows) {
    return section(
        "M",
        rows.map((r) => [r.timestamp, r.username]),
    );
}

const FORBIDDEN = ["multiSignature", "htlc.locks", "htlc.lockedBalance", "ipfs", "ipfs.hashes"];

/** The #W record of a wallet, or null when the wallet is not listed. */
function walletRecord(wallet) {
    for (const key of FORBIDDEN) {
        if (wallet.hasAttribute(key)) throw new Error(`wallet ${wallet.getAddress()} has the attribute ${key}, which no kept transaction type sets`);
    }
    const balance = wallet.getBalance();
    const nonce = wallet.getNonce();
    const publicKey = wallet.getPublicKey();
    const second = wallet.hasAttribute("secondPublicKey") ? wallet.getAttribute("secondPublicKey") : undefined;
    const isDelegate = wallet.hasAttribute("delegate.username");
    const votes = wallet.hasAttribute("votes") ? wallet.getAttribute("votes") : new Map();
    if (balance.isZero() && nonce.isZero() && publicKey === undefined && second === undefined && !isDelegate && votes.size === 0) return null;
    const r = { a: wallet.getAddress() };
    if (publicKey !== undefined) r.pk = publicKey;
    r.bal = balance.toFixed();
    r.n = nonce.toFixed();
    if (second !== undefined) r.spk = second;
    if (isDelegate) {
        const d = wallet.getAttribute("delegate");
        r.u = d.username;
        r.vb = amountText(d.voteBalance);
        r.vc = d.voters;
        if (d.rank !== undefined) r.rk = d.rank;
        if (d.resigned !== undefined) r.rs = d.resigned;
        r.pb = d.producedBlocks;
        r.ff = amountText(d.forgedFees);
        r.fr = amountText(d.forgedRewards);
        r.bf = amountText(d.burnedFees);
        r.dn = amountText(d.donations);
        if (d.lastBlock !== undefined) r.lb = d.lastBlock;
    }
    if (votes.size > 0) r.v = [...votes.entries()].map(([name, percent]) => [name, Math.round(percent * 100)]);
    return r;
}

function walletSection(wallets) {
    const records = [];
    for (const w of wallets) {
        const r = walletRecord(w);
        if (r) records.push(r);
    }
    records.sort((x, y) => byText(x.a, y.a));
    return section("W", records);
}

/** The hashes of five sections {B, S, R, M, W}, and the height digest. */
function hashes(sections) {
    const h = {};
    for (const k of ["B", "S", "R", "M", "W"]) h[k] = taggedHash(TAGS.section, Buffer.from(sections[k], "utf8"));
    const sd = taggedHash(TAGS.height, h.B, h.S, h.R, h.M, h.W);
    return { h, sd };
}

/** The five sections of a headless chain's state, with `missed` as the rows of this step. */
function chainSections(chain, missed) {
    const { BigNumber } = chain.lib.Utils;
    const { roundCalculator } = chain.pkgs.kernel.Utils;
    const last = chain.lastBlock();
    const height = last.data.height;
    const wallets = chain.wallets();
    const { forging, blocks } = chain.round();
    let active;
    if (height === 1 || roundCalculator.isNewRound(height + 1)) {
        active = chain.store.rounds.get(roundCalculator.calculateRound(height + 1).round) || [];
    }
    return {
        B: blockSection(last),
        S: supplySection(chain.stateStore.getGenesisIssuance(), wallets, BigNumber),
        R: roundSection(roundCalculator.calculateRound(height), forging, blocks, active),
        M: missedSection(missed),
        W: walletSection(wallets),
    };
}

/** The sections of a wallet fixture's state (no block, no round). */
function fixtureSections(wallets, BigNumber) {
    return {
        B: section("B", []),
        S: supplySection("0", wallets, BigNumber),
        R: section("R", []),
        M: section("M", []),
        W: walletSection(wallets),
    };
}

/** A replay's running digest: line() makes the next line and advances CD. */
class DigestChain {
    constructor() {
        this.cd = ZERO;
    }

    line({ height, step, id, reward }, sections) {
        const { h, sd } = hashes(sections);
        this.cd = taggedHash(TAGS.chain, this.cd, sd);
        const out = { h: height, step, id };
        if (reward !== undefined) out.rw = reward;
        out.b = h.B.toString("hex");
        out.s = h.S.toString("hex");
        out.r = h.R.toString("hex");
        out.m = h.M.toString("hex");
        out.w = h.W.toString("hex");
        out.sd = sd.toString("hex");
        out.cd = this.cd.toString("hex");
        return out;
    }
}

module.exports = {
    TAGS,
    DigestChain,
    blockSection,
    chainSections,
    fixtureSections,
    hashes,
    missedSection,
    roundSection,
    supplySection,
    taggedHash,
    walletRecord,
    walletSection,
};
