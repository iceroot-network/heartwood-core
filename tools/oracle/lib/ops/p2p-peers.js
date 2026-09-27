"use strict";
// Oracle operation of a ping's status attestations, over the reference's peer communicator
// (peer-communicator.ts, ping):
//   peer.attest   {status, maxDelegates?, peer, peers, lastBlock, slot, delegates}
//                 -> {attributed, versions, moved}: the signature loop of ping
//
// The reference's own PeerCommunicator.ping runs on the reply, with stubbed collaborators: emit
// answers with the decoded reply (after the reply schema, as emit checks it), the network and
// version checks pass, the verifier answers with a result forked or not as `peer.forked` says,
// the slot is `slot`, and the wallet repository holds the delegates. Everything the loop does is
// the reference's code.
//
// Inputs. `status` is a getStatus reply in hex; `maxDelegates` replaces the bound of its keys and
// signatures (the reference computes it from the milestones). `peer` is the pinged peer {ip,
// forked}. `peers` are the repository's other peers, in order: {ip, publicKeys, forked (null when
// never verified), header: {height, id} or null for a peer without a state}. `lastBlock` is our
// last block {height, id}; `slot` our current slot number; `delegates` the public keys of every
// registered delegate, in the order of allByUsername.
//
// Output. `attributed`: the pinged peer's keys after the loop. `versions`: the delegate.version
// values the loop set, in order, as {publicKey, round, version}. `moved`: each key taken from
// another peer, in order, as {ip, publicKey}.

const path = require("path");

module.exports = function p2pPeersOps({ solar, hexBuffer, safeInteger }) {
    const { Crypto, Validation } = solar.lib;
    const dist = path.join(solar.solarDir, "packages", "p2p", "dist");
    let reference;
    /** The communicator, the peer and the getStatus codec and reply schema, loaded on first use. */
    function load() {
        if (!reference) {
            reference = {
                PeerCommunicator: require(path.join(dist, "peer-communicator")).PeerCommunicator,
                Peer: require(path.join(dist, "peer")).Peer,
                PeerVerificationResult: require(path.join(dist, "peer-verifier")).PeerVerificationResult,
                codec: require(path.join(dist, "socket-server", "codecs")).Codecs.getStatus,
                schema: require(path.join(dist, "schemas")).replySchemas["p2p.peer.getStatus"],
            };
        }
        return reference;
    }

    /** A verification result that is forked or not: our height 10, theirs 12 or 10. */
    function verification(forked) {
        const { PeerVerificationResult } = load();
        return forked ? new PeerVerificationResult(10, 12, 8) : new PeerVerificationResult(10, 10, 10);
    }

    return {
        "peer.attest": async ({ status, maxDelegates, peer, peers, lastBlock, slot, delegates }) => {
            const { PeerCommunicator, Peer, codec } = load();
            const reply = codec.response.deserialise(hexBuffer(status, "status"));
            let schema = load().schema;
            if (maxDelegates !== undefined) {
                schema = JSON.parse(JSON.stringify(schema));
                schema.properties.publicKeys.maxItems = safeInteger(maxDelegates, "maxDelegates");
                schema.properties.signatures.maxItems = maxDelegates;
            }
            const { error } = Validation.validator.validate(schema, reply);
            if (error) throw new TypeError(`the status reply fails its schema: ${error}`);

            const pinged = new Peer(peer.ip, 4002);
            const moved = [];
            const others = peers.map(({ ip, publicKeys, forked, header }) => {
                const other = new Peer(ip, 4002);
                if (forked !== null) other.verificationResult = verification(forked);
                if (header !== null) other.state = { height: header.height, header: { ...header } };
                // Record each key the loop takes away, in order.
                let keys = [...publicKeys];
                Object.defineProperty(other, "publicKeys", {
                    get: () => keys,
                    set: (next) => {
                        for (const key of new Set(keys)) if (!next.includes(key)) moved.push({ ip, publicKey: key });
                        keys = next;
                    },
                });
                return other;
            });
            const versions = [];
            const wallets = new Map(
                delegates.map((key) => [
                    key,
                    {
                        getPublicKey: () => key,
                        isDelegate: () => true,
                        setAttribute: (name, value) => {
                            if (name !== "delegate.version") throw new Error(`unexpected attribute ${name}`);
                            versions.push({ publicKey: key, round: value.round, version: value.version });
                        },
                    },
                ]),
            );
            const verifier = {
                checkState: async () => verification(peer.forked),
                checkStateFast: async () => undefined,
            };
            const communicator = Object.create(PeerCommunicator.prototype);
            Object.assign(communicator, {
                app: {
                    version: () => "4.3.1",
                    resolve: () => ({ initialise: () => verifier }),
                    get: () => ({ getLastBlock: () => ({ data: { height: lastBlock.height, id: lastBlock.id } }) }),
                },
                repository: { getPeers: () => others },
                walletRepository: {
                    allByUsername: () => [...wallets.values()],
                    hasByPublicKey: (key) => wallets.has(key),
                    findByPublicKey: (key) => wallets.get(key),
                },
                emit: async () => reply,
                validatePeerConfig: () => true,
            });
            // The slot the loop compares with: `slot`, whatever the time.
            const { Slots } = Crypto;
            const getSlotInfo = Slots.getSlotInfo;
            Slots.getSlotInfo = () => ({ slotNumber: safeInteger(slot, "slot") });
            try {
                await communicator.ping(pinged, 60000, () => 0, true, false);
            } finally {
                Slots.getSlotInfo = getSlotInfo;
            }
            return { attributed: [...pinged.publicKeys], versions, moved };
        },
    };
};
