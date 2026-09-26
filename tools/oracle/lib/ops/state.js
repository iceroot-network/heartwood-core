"use strict";
// Oracle operations of the reference's wallets, ranking and rounds: state.shuffle, state.rank and
// wallet.voteAmounts. Loaded by lib/ops.js.
//
// Each operation runs the reference's own code on bare instances with stub collaborators, as the
// reference's own tests do: RoundState.shuffleDelegates, DposState.buildDelegateRanking and
// setDelegatesRound, and Wallet.calculateVoteAmount. Nothing here re-implements a rule.

const path = require("path");

// The largest delegate list an operation takes; the rules never see more than a few hundred.
const MAX_DELEGATES = 100000;
// The largest range of rounds one state.shuffle digest covers.
const MAX_ROUNDS = 100000;

module.exports = function stateOps({ solar, cm, describe, safeInteger, amount, sha256Hex, hexBuffer }) {
    const { Utils } = solar.lib;
    let reference;

    /** The reference's state classes, loaded on first use from the same build as the crypto library. */
    function classes() {
        if (!reference) {
            const load = (pkg, sub = "") => require(path.join(solar.solarDir, "packages", pkg, "dist", sub));
            const { RoundState } = load("state", "round-state");
            const { DposState } = load("state", "dpos/dpos");
            const { Wallets } = load("state");
            const { Services } = load("kernel");
            const shared = require(require.resolve("@solar-network/crypto", { paths: [path.join(solar.solarDir, "packages", "state")] }));
            if (shared !== solar.lib) throw new Error("@solar-network/state loaded a second copy of @solar-network/crypto");
            // The wallet attributes the delegate handlers register.
            const attributes = new Services.Attributes.AttributeSet();
            for (const key of ["votes", "delegate", "delegate.username", "delegate.voteBalance", "delegate.voters", "delegate.rank", "delegate.round", "delegate.resigned"]) {
                attributes.set(key);
            }
            reference = { RoundState, DposState, Wallet: Wallets.Wallet, AttributeMap: Services.Attributes.AttributeMap, attributes };
        }
        return reference;
    }

    const silentLogger = { debug() {}, info() {}, notice() {}, warning() {}, error() {} };

    function count(value, name, max) {
        safeInteger(value, name);
        if (value < 0 || value > max) throw new RangeError(`${name} must be from 0 to ${max}`);
        return value;
    }

    /** The input indices in the forging order of `round`, from the reference's shuffle. */
    function shuffled(roundState, round, n) {
        const stubs = Array.from({ length: n }, (_, index) => ({ index, clone: () => ({ index }) }));
        return roundState.shuffleDelegates({ round }, stubs).map((wallet) => wallet.index);
    }

    return {
        // The forging order of `count` delegates: for one round, the input index at each position;
        // for the rounds `from` to `to`, the SHA-256 of their orders as text, one line per round,
        // the indices separated by commas.
        "state.shuffle": ({ round, from, to, count: n }) => {
            count(n, "count", MAX_DELEGATES);
            const roundState = Object.create(classes().RoundState.prototype);
            if (round !== undefined) {
                return { order: shuffled(roundState, count(round, "round", Number.MAX_SAFE_INTEGER), n) };
            }
            count(from, "from", Number.MAX_SAFE_INTEGER);
            count(to, "to", Number.MAX_SAFE_INTEGER);
            if (from > to || to - from >= MAX_ROUNDS) throw new RangeError(`from <= to and at most ${MAX_ROUNDS} rounds`);
            let text = "";
            for (let r = from; r <= to; r++) text += `${shuffled(roundState, r, n).join(",")}\n`;
            return { rounds: to - from + 1, sha256: sha256Hex(Buffer.from(text, "utf8")) };
        },

        // buildDelegateRanking over the delegates, then setDelegatesRound with `maxDelegates` (by
        // default activeDelegates of the milestone at height 1). Each delegate: username, publicKey
        // (hex), voteBalance, and resigned (0 temporary, 1 permanent) when it has resigned. Output:
        // each delegate's rank (null for none) in input order, and the round's delegates in rank
        // order, or the error that setDelegatesRound raised. An error of the ranking is the
        // operation's error.
        "state.rank": ({ delegates, maxDelegates }) => {
            if (!Array.isArray(delegates) || delegates.length > MAX_DELEGATES) throw new TypeError(`delegates must be an array of at most ${MAX_DELEGATES}`);
            const max = maxDelegates === undefined ? cm.getMilestone(1).activeDelegates : count(maxDelegates, "maxDelegates", MAX_DELEGATES);
            const { DposState, Wallet, AttributeMap, attributes } = classes();
            const wallets = delegates.map(({ username, publicKey, voteBalance, resigned }, index) => {
                if (typeof username !== "string") throw new TypeError(`delegates[${index}].username must be text`);
                hexBuffer(publicKey, `delegates[${index}].publicKey`);
                const wallet = new Wallet(`delegate-${index}`, new AttributeMap(attributes), false);
                wallet.setPublicKey(publicKey);
                wallet.setAttribute("delegate", { username, voteBalance: Utils.BigNumber.make(amount(voteBalance, `delegates[${index}].voteBalance`)), voters: 0 });
                if (resigned !== undefined) {
                    if (resigned !== 0 && resigned !== 1) throw new TypeError(`delegates[${index}].resigned must be 0 or 1`);
                    wallet.setAttribute("delegate.resigned", resigned);
                }
                return wallet;
            });
            const dpos = Object.create(DposState.prototype);
            dpos.logger = silentLogger;
            dpos.walletRepository = { allByUsername: () => wallets };
            dpos.activeDelegates = [];
            dpos.roundDelegates = [];
            dpos.roundInfo = null;

            dpos.buildDelegateRanking();
            const ranks = wallets.map((wallet) => [
                wallet.getAttribute("delegate.username"),
                wallet.hasAttribute("delegate.rank") ? wallet.getAttribute("delegate.rank") : null,
            ]);
            let round;
            try {
                dpos.setDelegatesRound({ round: 1, roundHeight: 1, nextRound: 1, maxDelegates: max });
                round = dpos.getRoundDelegates().map((wallet) => wallet.getAttribute("delegate.username"));
            } catch (error) {
                round = { error: describe(error) };
            }
            return { ranks, round };
        },

        // Wallet.calculateVoteAmount of `balance` over `votes`, a list of [name, basis points] that
        // becomes the wallet's vote map in list order (the map holds percentages, basis points /
        // 100, as a wallet's votes do). Output: each delegate's share in map order.
        "wallet.voteAmounts": ({ balance, votes }) => {
            amount(balance, "balance");
            if (!Array.isArray(votes)) throw new TypeError("votes must be an array of [name, basis points]");
            const map = new Map(
                votes.map((entry, index) => {
                    if (!Array.isArray(entry) || entry.length !== 2 || typeof entry[0] !== "string") throw new TypeError(`votes[${index}] must be [name, basis points]`);
                    return [entry[0], count(entry[1], `votes[${index}] basis points`, 10000) / 100];
                }),
            );
            const wallet = Object.create(classes().Wallet.prototype);
            const shares = wallet.calculateVoteAmount({ balance: Utils.BigNumber.make(balance), lockedBalance: Utils.BigNumber.ZERO }, map);
            return {
                amounts: [...shares.entries()].map(([name, share]) => [name, share.balance.plus(share.lockedBalance).toString()]),
            };
        },
    };
};
