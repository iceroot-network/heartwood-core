"use strict";
// Oracle operation of the scenario compiler (tools/difftest scenario compile):
//   scenario.compile {crypto: {network, milestones, genesisBlock, exceptions}, transactions: [spec]}
//                    -> {transactions: [{id, hex}]}
// Every transaction is built once, in list order, under the chain's own configuration, at the
// height its spec plans it for: a spec is {kind, passphrase, secondPassphrase? (a second signature
// of the sender; a registration takes it in its asset instead), nonce, height, fee?, memo?,
// asset}, its fee the network's dynamic floor at `height` when absent. The bytes are
// read back with the strict decoder at `height` before they are answered, so an implementation
// under test and the reference are given the same bytes and the same id. Signing uses the fixed aux
// of lib/ops.js. Loaded by lib/ops.js.

const KINDS = new Set(["transfer", "secondSignature", "vote", "delegateRegistration", "delegateResignation", "burn", "ipfs"]);

// A fee to build with before the floor is known: its bytes are as long as any fee's.
const PLACEHOLDER_FEE = "10000000000";

module.exports = function scenarioFile(context) {
    const { solar, devnet, cm, atHeight, handlerRegistry, safeInteger } = context;
    const { Transactions } = solar.lib;

    const text = (value, name) => {
        if (typeof value !== "string") throw new TypeError(`${name} must be text`);
        return value;
    };

    /** One builder with the calls of a spec, signed (and second-signed) and built at `fee`. */
    function make(spec, fee) {
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
        b = b.sign(text(spec.passphrase, "passphrase"));
        if (spec.kind !== "secondSignature" && spec.secondPassphrase !== undefined && spec.secondPassphrase !== null) b = b.secondSign(text(spec.secondPassphrase, "secondPassphrase"));
        return b.build();
    }

    /** The transaction of a spec at its planned height: the given fee, else the dynamic floor. */
    function build(spec) {
        if (spec === null || typeof spec !== "object") throw new TypeError("a transaction spec must be an object");
        if (!KINDS.has(spec.kind)) throw new TypeError(`unknown kind ${spec.kind}`);
        const height = safeInteger(spec.height, "height");
        return atHeight(height, () => {
            if (spec.fee !== undefined && spec.fee !== null) return make(spec, text(spec.fee, "fee"));
            const draft = make(spec, PLACEHOLDER_FEE);
            const { InternalTransactionType } = Transactions;
            const handler = handlerRegistry().getRegisteredHandlerByType(InternalTransactionType.from(draft.data.type, draft.data.typeGroup));
            return make(spec, handler.getMinimumFee(draft, cm.getMilestone(height).dynamicFees).toString());
        });
    }

    /** The strict decode of the built bytes at the planned height; its id must be the built one. */
    function strict(tx, height) {
        const back = atHeight(height, () => Transactions.TransactionFactory.fromBytes(tx.serialised, true));
        if (back.id !== tx.id) throw new Error(`the strict decode of ${tx.id} gives ${back.id}`);
    }

    return {
        "scenario.compile": ({ crypto, transactions }) => {
            if (!crypto || typeof crypto !== "object") throw new TypeError("scenario.compile needs the chain's crypto files");
            if (!Array.isArray(transactions)) throw new TypeError("transactions must be a list of specs");
            cm.setConfig(JSON.parse(JSON.stringify(crypto)));
            try {
                return {
                    transactions: transactions.map((spec) => {
                        const tx = build(spec);
                        strict(tx, spec.height);
                        return { id: tx.id, hex: tx.serialised.toString("hex") };
                    }),
                };
            } finally {
                devnet.restore();
            }
        },
    };
};
