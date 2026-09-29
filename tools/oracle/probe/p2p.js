"use strict";
// The reference-client probe: the reference's own peer client (its nes client, PeerConnector,
// PeerCommunicator, the codecs and the reply schemas) run against one or more relays, case by case
// from the P2P corpus. One JSON line per case and target goes to OUT/<target>/cases.jsonl.
//
//   node p2p.js --cases FILE --crypto DIR --out DIR [--wallets FILE] [--only ID,ID] NAME=IP[:PORT] ...
//
// The targets are asked in lock step, so both answers of a case fall in one slot. The probe
// announces the headers {port: 6002, version: "4.3.1"}: nothing listens on that port, so no target
// can accept the probe as a peer. PROBE_SEED (default "probe") seeds the fresh transfer's signature.
// Reads SOLAR_DIR (the built reference; the oracle image sets it).

const crypto = require("crypto");
const fs = require("fs");
const http = require("http");
const path = require("path");

const USAGE = "usage: node probe/p2p.js --cases FILE --crypto DIR --out DIR [--wallets FILE] [--only ID,ID] NAME=IP[:PORT]...";
const P2P_PORT = 6002;
const SOLAR = process.env.SOLAR_DIR;
if (!SOLAR) fail("SOLAR_DIR is not set (the built reference)");
const pkg = (name, ...rest) => path.join(SOLAR, "packages", name, ...rest);

function fail(message) {
    process.stderr.write(`${message}\n${USAGE}\n`);
    process.exit(2);
}

function parseArgs(argv) {
    const opts = { targets: [], only: null, wallets: null };
    for (let i = 0; i < argv.length; i++) {
        const a = argv[i];
        if (a === "--cases") opts.cases = argv[++i];
        else if (a === "--crypto") opts.crypto = argv[++i];
        else if (a === "--out") opts.out = argv[++i];
        else if (a === "--wallets") opts.wallets = argv[++i];
        else if (a === "--only") opts.only = new Set(String(argv[++i]).split(","));
        else if (/^[\w.-]+=[^=]+$/.test(a)) {
            const [name, addr] = a.split("=");
            const [ip, port] = addr.split(":");
            opts.targets.push({ name, ip, port: Number(port || P2P_PORT) });
        } else fail(`unknown argument ${a}`);
    }
    if (!opts.cases || !opts.crypto || !opts.out || opts.targets.length === 0) fail("missing argument");
    return opts;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sha256 = (buf) => crypto.createHash("sha256").update(buf).digest("hex");

// ---- the reference's own code -------------------------------------------------------------------

const { Client } = require(pkg("nes", "dist"));
const solarCrypto = require(pkg("crypto", "dist"));
const p2p = require(pkg("p2p", "dist"));
const { Codecs } = p2p;
const { replySchemas } = require(pkg("p2p", "dist", "schemas"));
const { PeerCommunicator } = require(pkg("p2p", "dist", "peer-communicator"));
const { PeerConnector } = require(pkg("p2p", "dist", "peer-connector"));
const { Peer } = require(pkg("p2p", "dist", "peer"));
const { stringify } = require(pkg("kernel", "dist", "utils", "stringify"));
const { parseNesMessage } = require(pkg("nes", "dist", "utils"));
const getCodecModule = require(pkg("p2p", "dist", "socket-server", "utils", "get-codec"));
const { Enums } = require(pkg("kernel", "dist"));

const CODEC_OF = {
    "p2p.peer.getStatus": "getStatus",
    "p2p.peer.getPeers": "getPeers",
    "p2p.peer.getCommonBlocks": "getCommonBlocks",
    "p2p.blocks.getBlocks": "getBlocks",
    "p2p.blocks.postBlock": "postBlock",
    "p2p.transactions.getUnconfirmedTransactions": "getUnconfirmedTransactions",
    "p2p.transactions.postTransactions": "postTransactions",
    "p2p.internal.getCurrentRound": "getCurrentRound",
};
// The communicator resolves codecs through the application's routes; the probe has no application.
getCodecModule.getCodec = (_app, event) => Codecs[CODEC_OF[event]];

const HEADERS = { port: P2P_PORT, version: "4.3.1" };

function plain(x) {
    return JSON.parse(
        JSON.stringify(x, (k, v) => {
            if (v && v.type === "Buffer" && Array.isArray(v.data)) return Buffer.from(v.data).toString("hex");
            if (typeof v === "bigint") return String(v);
            if (v && typeof v === "object" && v.constructor && v.constructor.name === "BigNumber") return v.toString();
            return v;
        }),
    );
}

function errInfo(e) {
    if (!e) return null;
    return {
        name: e.name || null,
        message: e.message || String(e),
        type: e.type || null,
        statusCode: e.statusCode || null,
        data: typeof e.data === "string" ? e.data : null,
    };
}

function verdict(endpoint, reply) {
    const schema = replySchemas[endpoint];
    if (!schema) return "no schema";
    const { error } = solarCrypto.Validation.validator.validate(schema, reply);
    return error ? String(error) : "valid";
}

/** A shorter record of a long answer: the listing of a getBlocks reply, not its bytes. */
function summarise(endpoint, decoded) {
    if (endpoint === "p2p.blocks.getBlocks" && Array.isArray(decoded)) {
        return { count: decoded.length, blocks: decoded.map((b) => ({ id: b.id, height: b.height, transactions: b.numberOfTransactions })) };
    }
    return decoded;
}

// ---- one target ----------------------------------------------------------------------------------

class Target {
    constructor({ name, ip, port }, opts) {
        Object.assign(this, { name, ip, port, opts });
        this.calls = [];
        this.log = [];
        this.facts = null;
        this.status = null;
        this.disconnects = [];
        this.serverPings = 0;
        this.outDir = path.join(opts.out, name);
        fs.mkdirSync(this.outDir, { recursive: true });
        this.file = path.join(this.outDir, "cases.jsonl");
        fs.writeFileSync(this.file, "");
        this.build();
    }

    /** The reference's connector and communicator, built without an application around them. */
    build() {
        const events = {
            handlers: [],
            listen: (name, handler) => events.handlers.push({ name, handler }),
            dispatch: (name, data) => {
                if (name === Enums.PeerEvent.Disconnect) this.connector.disconnect(data.peer);
            },
        };
        const logger = {};
        for (const level of ["debug", "info", "warning", "error", "notice"]) logger[level] = (m) => this.log.push(`${level}: ${m}`);
        const connector = Object.create(PeerConnector.prototype);
        Object.assign(connector, { logger, connections: new Map(), errors: new Map(), lastConnectionCreate: new Map() });
        const self = this;
        const emit = connector.emit.bind(connector);
        connector.emit = async (peer, event, payload, timeout) => {
            const rec = { endpoint: event, requestHex: Buffer.from(payload).toString("hex").slice(0, 131072), requestSha256: sha256(payload), at: Date.now() };
            self.calls.push(rec);
            try {
                const r = await emit(peer, event, payload, timeout);
                rec.ms = Date.now() - rec.at;
                rec.raw = Buffer.from(r.payload);
                return r;
            } catch (e) {
                rec.ms = Date.now() - rec.at;
                rec.error = errInfo(e);
                throw e;
            }
        };
        const connect = connector.connect.bind(connector);
        connector.connect = async (peer, maxPayload) => {
            const had = connector.connection(peer);
            try {
                const c = await connect(peer, maxPayload);
                if (!had) self.watch(c);
                return c;
            } catch (e) {
                self.calls.push({ endpoint: "connect", error: errInfo(e), at: Date.now() });
                throw e;
            }
        };
        this.connector = connector;
        const configuration = {
            getOptional: (k, d) => ({ rateLimit: 100, rateLimitPostTransactions: 25, minimumVersions: [">=4.0.0"] })[k] ?? d,
            getRequired: (k) => ({ getBlocksTimeout: 30000, maxPeerSequentialErrors: 3 })[k],
            get: (k) => (k === "server.port" ? P2P_PORT : undefined),
        };
        const app = { version: () => "4.3.1", getTagged: () => configuration };
        const communicator = Object.create(PeerCommunicator.prototype);
        const queue = { resume() {}, push(job) { self.lastQueue = job.handle(); } };
        Object.assign(communicator, {
            app, configuration, connector, events, logger,
            repository: { getPeers: () => [] },
            createQueue: async () => queue,
            stateStore: { getBlockchain: () => ({ value: "running" }) },
            walletRepository: {},
            postTransactionsQueueByIp: new Map(),
        });
        communicator.initialise();
        this.communicator = communicator;
        this.peer = new Peer(this.ip, this.port);
    }

    /** Count the server's pings and the connection's ends on a client the connector made. */
    watch(client) {
        client.onDisconnect = (willReconnect, log) => this.disconnects.push({ willReconnect, log: plain(log) });
        const onMessage = client._onMessage.bind(client);
        client._onMessage = (message) => {
            try {
                if (parseNesMessage(message.data).type === "ping") this.serverPings++;
            } catch {
                // the client reports it
            }
            return onMessage(message);
        };
    }

    /** The recorded calls since the last take, with each raw answer decoded and checked. */
    take() {
        const calls = this.calls.splice(0).map((c) => {
            const out = { endpoint: c.endpoint, requestHex: c.requestHex, requestSha256: c.requestSha256, ms: c.ms ?? null, error: c.error || null };
            if (c.raw) {
                out.responseSha256 = sha256(c.raw);
                out.responseLength = c.raw.length;
                const codec = Codecs[CODEC_OF[c.endpoint]];
                try {
                    const decoded = codec.response.deserialise(c.raw);
                    out.verdict = verdict(c.endpoint, decoded);
                    out.answer = summarise(c.endpoint, plain(decoded));
                } catch (e) {
                    out.decodeError = String(e);
                }
            }
            return out;
        });
        const log = this.log.splice(0);
        return { calls, log };
    }

    socketState() {
        const c = this.connector.connection(this.peer);
        if (!c) return "none";
        return c._ws && c._ws.readyState === 1 ? "open" : "closed";
    }

    write(record) {
        fs.appendFileSync(this.file, `${JSON.stringify({ target: this.name, ip: this.ip, ...record })}\n`);
    }

    /** Ask through the communicator: its own reply check and error handling. */
    async status1() {
        const r = await this.communicator.emit(this.peer, "p2p.peer.getStatus", {}, 5000);
        return r;
    }

    async apiGet(port, urlPath) {
        return new Promise((resolve) => {
            http
                .get({ host: this.ip, port, path: urlPath, timeout: 3000 }, (res) => {
                    let s = "";
                    res.on("data", (d) => (s += d));
                    res.on("end", () => {
                        try { resolve(JSON.parse(s)); } catch { resolve(null); }
                    });
                })
                .on("error", () => resolve(null));
        });
    }

    async apiHead(port) {
        return new Promise((resolve) => {
            const req = http.request({ host: this.ip, port, path: "/", method: "HEAD", timeout: 3000 }, (res) => {
                res.resume();
                resolve(res.statusCode);
            });
            req.on("error", () => resolve(null));
            req.on("timeout", () => req.destroy());
            req.end();
        });
    }

    /** A new plain client for the refusal cases: no reconnection, the reference's nes client. */
    async rawClient(timeout = 5000) {
        const client = new Client(`ws://${this.ip}:${this.port}`, { ws: { maxPayload: 20971520 }, timeout: 10000 });
        client.onError = () => {};
        const closed = { at: null, code: null };
        this.serverPings = 0;
        const t0 = Date.now();
        await client.connect({ timeout, reconnect: false });
        client.helloMs = Date.now() - t0;
        this.watch(client);
        client.onDisconnect = (will, log) => { closed.at = Date.now(); closed.log = plain(log); };
        client.closed = closed;
        return client;
    }
}

// ---- values a case names ---------------------------------------------------------------------------

function resolve(value, facts) {
    if (typeof value === "string" && value.startsWith("$")) {
        const v = value.slice(1).split(".").reduce((o, k) => (o == null ? o : o[k]), facts);
        if (v === undefined) throw new Error(`unknown value ${value}`);
        return v;
    }
    if (Array.isArray(value)) return value.flatMap((x) => {
        const r = resolve(x, facts);
        return typeof x === "string" && x.startsWith("$") && Array.isArray(r) ? r : [r];
    });
    if (value && typeof value === "object") {
        if (typeof value.$ === "string") {
            let n = resolve(`$${value.$}`, facts);
            if (value.div) n = Math.floor(n / value.div);
            if (value.add) n += value.add;
            return Math.max(value.min ?? -Infinity, n);
        }
        return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, resolve(v, facts)]));
    }
    return value;
}

// ---- the fresh transfer --------------------------------------------------------------------------

/** One v3 transfer from a genesis wallet, signed with the reference's own signer (BIP340). */
let freshOnce;
/** The transfer is built once per run, so every target is sent the same bytes. */
function freshTransfer(t, opts) {
    freshOnce ||= buildFreshTransfer(t, opts);
    return freshOnce;
}

async function buildFreshTransfer(t, opts) {
    if (!opts.wallets) throw new Error("no --wallets file for the fresh transfer");
    const wallets = JSON.parse(fs.readFileSync(opts.wallets, "utf8"));
    const from = wallets.genesis[0];
    const to = wallets.genesis[1];
    const { Identities, Managers, Transactions, Utils, Crypto } = solarCrypto;
    const apiPort = apiPortOf(t.status);
    const wallet = await t.apiGet(apiPort, `/api/wallets/${from.address}`);
    let base = wallet && wallet.data && wallet.data.nonce !== undefined ? BigInt(wallet.data.nonce) : 0n;
    // Transactions of this sender already waiting in the pool (an earlier run) take the next nonces.
    const waiting = await t.apiGet(apiPort, `/api/transactions/unconfirmed?senderPublicKey=${from.publicKey}&limit=100`);
    if (waiting && waiting.meta && Number.isInteger(waiting.meta.totalCount)) base += BigInt(waiting.meta.totalCount);
    const keys = Identities.Keys.fromPassphrase(from.passphrase);
    const seed = process.env.PROBE_SEED || "probe";
    const aux = crypto.createHash("sha256").update(`p2p-probe/${seed}/transfer`).digest();
    const BN = (v) => Utils.BigNumber.make(String(v));
    const ms = Managers.configManager.getMilestone();
    const build = (fee) => {
        const data = {
            version: 3, network: Managers.configManager.get("network.pubKeyHash"), typeGroup: 1, type: 6,
            nonce: BN(base + 1n), senderPublicKey: from.publicKey, fee: BN(fee),
            asset: { transfers: [{ amount: BN(123456789), recipientId: to.address }] },
        };
        const opt = { excludeSignature: true, excludeSecondSignature: true, disableVersionCheck: true };
        data.signature = Crypto.Hash.signSchnorrBip340(Transactions.Utils.toHash(data, opt), keys, aux);
        data.id = Transactions.Utils.toHash(data, { disableVersionCheck: true }).toString("hex");
        return { data, bytes: Transactions.Utils.toBytes(data) };
    };
    const staticFee = BigInt((ms.fees && ms.fees.staticFees && ms.fees.staticFees.transfer) || 0);
    let min = 0n;
    if (ms.dynamicFees && ms.dynamicFees.enabled) {
        const size = build(0).bytes.length;
        min = (BigInt(ms.dynamicFees.addonBytes.transfer || 0) + BigInt(Math.round(size / 2))) * BigInt(ms.dynamicFees.minFee);
    }
    return build(staticFee > min ? staticFee : min).bytes;
}

/** A transaction that is already in the chain: the first of the genesis block, as canonical bytes. */
function forgedTransfer(opts) {
    const genesis = JSON.parse(fs.readFileSync(path.join(opts.crypto, "genesisBlock.json"), "utf8"));
    const { Transactions } = solarCrypto;
    const tx = genesis.transactions.find((x) => x.type === 6 && x.typeGroup === 1) || genesis.transactions[0];
    return Transactions.TransactionFactory.fromJson(tx).serialised;
}

function apiPortOf(status) {
    const plugin = status && status.config && status.config.plugins && status.config.plugins["@solar-network/api"];
    return plugin ? plugin.port : null;
}

// ---- the cases -----------------------------------------------------------------------------------

async function runCase(t, c, opts, settings) {
    const t0 = Date.now();
    const record = { case: c.id, kind: c.kind, at: new Date(t0).toISOString() };
    try {
        await CASES[c.kind](t, c, record, opts, settings);
    } catch (e) {
        record.probeError = String((e && e.stack) || e);
    }
    const { calls, log } = t.take();
    record.calls = calls;
    if (log.length) record.log = log;
    record.ms = Date.now() - t0;
    t.write(record);
    return record;
}

async function refuse(t, record, action, settings) {
    const client = await t.rawClient();
    let outcome;
    try {
        outcome = await action(client);
    } catch (e) {
        outcome = { error: errInfo(e) };
    }
    record.outcome = outcome;
    const deadline = Date.now() + 2000;
    while (Date.now() < deadline && client._ws && client._ws.readyState === 1) await sleep(100);
    record.socket = client._ws && client._ws.readyState === 1 ? "open" : "closed";
    record.disconnects = client.closed.at ? [client.closed.log] : [];
    client.terminate().catch(() => {});
    // A new connection at once: is the caller refused?
    const ban = { connect: null, request: null };
    let second;
    try {
        second = await t.rawClient(3000);
        ban.connect = "ok";
        try {
            const payload = Codecs.getStatus.request.serialise({ headers: HEADERS });
            await second.request({ path: "p2p.peer.getStatus", payload });
            ban.request = "answered";
        } catch (e) {
            ban.request = errInfo(e);
        }
    } catch (e) {
        ban.connect = errInfo(e);
    }
    if (second) second.terminate().catch(() => {});
    record.ban = ban;
    record.banned = ban.connect !== "ok" || ban.request !== "answered";
    await sleep(settings.refusalWaitMs);
}

const CASES = {
    async hello(t, c, record) {
        const client = await t.connector.connect(t.peer, 20971520);
        record.hello = { socketId: client.id, heartbeat: client._heartbeatTimeout };
        record.socket = t.socketState();
    },

    async ping(t, c, record) {
        const status = await t.status1();
        if (status) {
            t.status = status;
            const peer = t.peer;
            record.configValid = t.communicator.validatePeerConfig(peer, status.config);
            const stateBuffer = Buffer.from(stringify({ state: status.state, config: status.config }));
            const keys = status.publicKeys || [];
            const sigs = status.signatures || [];
            let verified = 0;
            for (let i = 0; i < sigs.length; i++) {
                try {
                    if (solarCrypto.Crypto.Hash.verifySchnorr(stateBuffer, sigs[i], keys[i])) verified++;
                } catch {
                    // counts as not verified
                }
            }
            record.attestations = { signed: sigs.length, verified };
            solarCrypto.Managers.configManager.setHeight(status.state.header.height);
            const statusCalls = t.calls.splice(0);
            const tip = status.state.header;
            const genesis = JSON.parse(fs.readFileSync(path.join(t.opts.crypto, "genesisBlock.json"), "utf8"));
            const blocks = await t.communicator.getPeerBlocks(t.peer, { fromBlockHeight: Math.max(1, tip.height - 10), blockLimit: 10, headersOnly: true, silent: true });
            t.take();
            t.tipHeader = blocks[blocks.length - 1];
            t.calls.push(...statusCalls);
            t.facts = {
                tip: { id: tip.id, height: tip.height },
                genesis: { id: genesis.id },
                unknown: { id: `${"0".repeat(63)}1` },
                ten: { ids: blocks.map((b) => b.id) },
                api: { port: apiPortOf(status) },
            };
        }
        record.socket = t.socketState();
    },

    async pingPorts(t, c, record) {
        t.peer.plugins = (t.status && t.status.config.plugins) || {};
        await t.communicator.pingPorts(t.peer);
        record.ports = { ...t.peer.ports };
        record.configuredPorts = Object.fromEntries(Object.entries(t.peer.plugins).map(([k, v]) => [k, v.port]));
    },

    async call(t, c, record, opts) {
        const args = resolve(c.args || [], t.facts);
        let result;
        const m = c.method;
        if (m === "getPeers") result = await t.communicator.getPeers(t.peer, true);
        else if (m === "hasCommonBlocks") result = await t.communicator.hasCommonBlocks(t.peer, args[0]);
        else if (m === "getPeerBlocks") result = await t.communicator.getPeerBlocks(t.peer, { ...args[0], silent: true });
        else if (m === "getUnconfirmedTransactions") result = await t.communicator.getUnconfirmedTransactions(t.peer, args[0] || []);
        else if (m === "postTransactions") {
            const bytes = c.transaction === "fresh" ? await freshTransfer(t, opts) : forgedTransfer(opts);
            await t.communicator.postTransactions(t.peer, [bytes]);
            await t.lastQueue;
        } else if (m === "postBlock") {
            const [block] = await t.communicator.getPeerBlocks(t.peer, { fromBlockHeight: t.facts.tip.height - 1, blockLimit: 1, silent: true });
            const txs = (block.transactions || []).map((data) => ({ data }));
            await t.communicator.postBlock(t.peer, { data: block, transactions: txs });
        } else throw new Error(`unknown method ${m}`);
        record.accepted = result !== undefined;
        record.socket = t.socketState();
    },

    /** A held connection: the reference client answers the server's pings for `seconds`. */
    async idle(t, c, record) {
        // The server keeps one connection per address (a newer one ends the older), and it bans an
        // address whose connection closes without a request: end the shared connection, ask once.
        t.connector.disconnect(t.peer);
        const client = await t.rawClient();
        record.hello = { socketId: client.id, heartbeat: client._heartbeatTimeout };
        await client.request({ path: "p2p.peer.getStatus", payload: Codecs.getStatus.request.serialise({ headers: HEADERS }) });
        await sleep(c.seconds * 1000);
        record.pings = t.serverPings;
        record.pingsSeen = t.serverPings >= 2;
        record.socket = client._ws && client._ws.readyState === 1 ? "open" : "closed";
        record.disconnects = client.closed.at ? [client.closed.log] : [];
        client.terminate().catch(() => {});
    },

    /** One request on a new connection that the target must refuse; then the ban check and the wait. */
    async refusal(t, c, record, opts, settings) {
        t.connector.disconnect(t.peer);
        await refuse(t, record, async (client) => {
            if (c.frame === "text") {
                client._ws.send("not a nes message");
                return { sent: "text frame" };
            }
            let payload;
            if (c.payloadHex !== undefined) payload = Buffer.from(c.payloadHex, "hex");
            else if (c.postBlockOversized) {
                const { Blocks, Managers } = solarCrypto;
                const block = t.tipHeader;
                const max = Managers.configManager.getMilestone().block.maxTransactions;
                const bytes = Blocks.Serialiser.serialise({ ...block, numberOfTransactions: max + 1 }, true);
                payload = Codecs.postBlock.request.serialise({ block: bytes, headers: HEADERS });
            } else {
                const codec = Codecs[CODEC_OF[c.path]];
                payload = codec.request.serialise({ ...resolve(c.body || {}, t.facts), headers: c.headers || HEADERS });
            }
            const rec = { endpoint: c.path, requestHex: payload.toString("hex").slice(0, 131072), requestSha256: sha256(payload), at: Date.now() };
            try {
                const r = await client.request({ path: c.path, payload });
                rec.raw = Buffer.from(r.payload);
                t.calls.push(rec);
                return { answered: true };
            } catch (e) {
                rec.error = errInfo(e);
                rec.ms = Date.now() - rec.at;
                t.calls.push(rec);
                return { answered: false, error: errInfo(e) };
            }
        }, settings);
    },
};

async function main() {
    const opts = parseArgs(process.argv.slice(2));
    const load = (f) => JSON.parse(fs.readFileSync(path.join(opts.crypto, f), "utf8"));
    solarCrypto.Managers.configManager.setConfig({
        network: load("network.json"), milestones: load("milestones.json"),
        exceptions: load("exceptions.json"), genesisBlock: load("genesisBlock.json"),
    });
    solarCrypto.Managers.configManager.setHeight(1);
    const corpus = JSON.parse(fs.readFileSync(opts.cases, "utf8"));
    const settings = { settleMs: corpus.settleMs ?? 500, refusalWaitMs: corpus.refusalWaitMs ?? 11000 };
    const targets = opts.targets.map((x) => new Target(x, opts));
    for (const c of corpus.cases) {
        if (opts.only && !opts.only.has(c.id) && c.kind !== "hello" && c.kind !== "ping") continue;
        const records = await Promise.all(targets.map((t) => runCase(t, c, opts, settings)));
        process.stderr.write(`${c.id}: ${records.map((r) => (r.probeError ? "probe error" : `${r.ms} ms`)).join(", ")}\n`);
        await sleep(settings.settleMs);
    }
    for (const t of targets) for (const conn of t.connector.all()) conn.terminate().catch(() => {});
    await sleep(200);
    process.exit(0);
}

main().catch((e) => {
    process.stderr.write(`p2p probe: ${(e && e.stack) || e}\n`);
    process.exit(1);
});
