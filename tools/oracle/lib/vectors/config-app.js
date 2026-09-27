"use strict";
// V28, the node's configuration: config.pluginOptions for the p2p, blockchain, pool, api and
// database packages (defaults, the variables their defaults read, numbers and booleans as text,
// arrays replaced, unknown keys, and one refusal per rule) and config.loadApp (the three profiles
// of a generated app.json, flags and services, the file order, an unknown package, a package with
// no service provider, a missing profile and each schema rule broken once).
//
// Every case's environment names the user, as a node's process does: the reference's database
// options take their user name from USER and refuse a start without it, which the relay does not
// need. Addresses are documentation ranges.

// The process of a relay: the variables the reference's kernel sets before the defaults load.
const BASE = { USER: "relay", CORE_TOKEN: "solar", CORE_NETWORK_NAME: "testnet", CORE_PATH_DATA: "/var/lib/relay/data" };

const P2P = "@solar-network/p2p";
const BLOCKCHAIN = "@solar-network/blockchain";
const POOL = "@solar-network/pool";
const API = "@solar-network/api";
const DATABASE = "@solar-network/database";

/** The profiles of a generated app.json, with `relay` in place of the relay's plugins. */
function appJson(relay, extra = {}) {
    const entries = (names) => names.map((name) => ({ package: `@solar-network/${name}` }));
    return {
        core: { plugins: entries(["logger", "state", "database", "transactions", "pool", "p2p", "blockchain", "api", "webhooks", "forger"]) },
        relay: { plugins: relay },
        forger: { plugins: entries(["logger", "transactions", "forger"]) },
        snapshot: { plugins: entries(["logger", "database", "snapshots"]) },
        ...extra,
    };
}

const RELAY = [
    { package: "@solar-network/logger" },
    { package: "@solar-network/state" },
    { package: "@solar-network/database" },
    { package: "@solar-network/transactions" },
    { package: "@solar-network/pool" },
    { package: P2P, options: { remoteAccess: ["127.0.0.1", "::ffff:127.0.0.1", "::1"], dns: [], ntp: [] } },
    { package: BLOCKCHAIN },
    { package: API },
    { package: "@solar-network/webhooks" },
];

module.exports = async function configAppVectors({ rec, shared }) {
    const out = [];
    const options = async (name, pkg, opts, env = {}) => {
        out.push({ ...(await rec("config.pluginOptions", shared.height, { package: pkg, options: opts, env: { ...BASE, ...env } })), name });
    };
    const app = async (name, text, role = "relay") => {
        out.push({ ...(await rec("config.loadApp", shared.height, { app: text, role })), name });
    };

    // ---- p2p ---------------------------------------------------------------------------------------
    await options("p2p defaults", P2P, {});
    await options("p2p generated devnet options", P2P, RELAY[5].options);
    await options("p2p from the variables", P2P, {}, {
        CORE_P2P_PORT: "6002",
        CORE_P2P_HOST: "0.0.0.0",
        CORE_P2P_BAN_SECONDS: "5",
        CORE_P2P_MIN_NETWORK_REACH: "1",
        CORE_P2P_MAX_PEERS_SAME_SUBNET: "10",
        CORE_P2P_MAX_PEER_SEQUENTIAL_ERRORS: "5",
        CORE_P2P_RATE_LIMIT: "20",
        CORE_P2P_RATE_LIMIT_POST_TRANSACTIONS: "5",
        CORE_P2P_ALLOW_PRIVATE_IP_RANGES: "true",
    });
    await options("p2p empty variables keep the defaults", P2P, {}, { CORE_P2P_PORT: "", CORE_P2P_ALLOW_PRIVATE_IP_RANGES: "" });
    await options("p2p options over the variables", P2P, { server: { port: 7002 }, minimumNetworkReach: 3 }, { CORE_P2P_PORT: "6002", CORE_P2P_MIN_NETWORK_REACH: "1" });
    await options("p2p numbers and booleans as text", P2P, {
        rateLimit: "50",
        rateLimitPostTransactions: " 7 ",
        verifyTimeout: "1e4",
        getBlocksTimeout: "20000.0",
        maxSameSubnetPeers: "020",
        networkStart: "true",
        disableDiscovery: "FALSE",
        skipDiscovery: true,
        ignoreMinimumNetworkReach: "True",
    });
    await options("p2p arrays replaced, unknown keys kept", P2P, { whitelist: ["192.0.2.*"], blacklist: ["198.51.100.1"], minimumVersions: [">=4.3.0"], gossip: { any: 1 } });
    await options("p2p empty lists", P2P, { whitelist: [], minimumVersions: [] });
    await options("p2p address forms", P2P, { server: { hostname: "::" }, remoteAccess: ["10.0.0.0/8", "2001:db8::/32"], dns: ["192.0.2.53"] });
    for (const [name, opts, env] of [
        ["port 0", { server: { port: 0 } }],
        ["port 65536", { server: { port: 65536 } }],
        ["port as a word", { server: { port: "http" } }],
        ["port from a bad variable", {}, { CORE_P2P_PORT: "http" }],
        ["reach -1 from a variable", {}, { CORE_P2P_MIN_NETWORK_REACH: "-1" }],
        ["reach 1.5", { minimumNetworkReach: 1.5 }],
        ["reach as a fraction", { minimumNetworkReach: "1.50" }],
        ["reach -0", { minimumNetworkReach: "-0" }],
        ["rate limit 0", { rateLimit: 0 }],
        ["whitelist not a list", { whitelist: "*" }],
        ["blacklist with an empty entry", { blacklist: [""] }],
        ["remote access by name", { remoteAccess: ["localhost"] }],
        ["dns by name", { dns: ["dns.example"] }],
        ["hostname by name", { server: { hostname: "localhost" } }],
        ["network start as a word", { networkStart: "yes" }],
        ["minimum versions null", { minimumVersions: null }],
        ["ban seconds -1", { server: { banSeconds: -1 } }],
    ]) {
        await options(`p2p refused: ${name}`, P2P, opts, env);
    }

    // ---- blockchain --------------------------------------------------------------------------------
    await options("blockchain defaults", BLOCKCHAIN, {});
    await options("blockchain rollback bounds as text", BLOCKCHAIN, { databaseRollback: { maxBlockRewind: "500", steps: "10" } });
    await options("blockchain network start", BLOCKCHAIN, { networkStart: "true", missedBlocksLookback: 100 });
    await options("blockchain network start off", BLOCKCHAIN, { networkStart: false });
    for (const [name, opts] of [
        ["rewind 0", { databaseRollback: { maxBlockRewind: 0 } }],
        ["steps as a word", { databaseRollback: { steps: "x" } }],
        ["rollback null", { databaseRollback: null }],
        ["network start as a word", { networkStart: "yes" }],
    ]) {
        await options(`blockchain refused: ${name}`, BLOCKCHAIN, opts);
    }

    // ---- pool --------------------------------------------------------------------------------------
    const sender = shared.recipients[0];
    await options("pool defaults", POOL, {});
    await options("pool from the variables", POOL, {}, {
        CORE_MAX_TRANSACTIONS_IN_POOL: "500",
        CORE_TRANSACTION_POOL_MAX_PER_SENDER: " 20 ",
        CORE_TRANSACTION_POOL_MAX_PER_REQUEST: "10",
        CORE_TRANSACTION_POOL_DISABLED: "1",
    });
    await options("pool empty variables keep the defaults", POOL, {}, { CORE_MAX_TRANSACTIONS_IN_POOL: "", CORE_TRANSACTION_POOL_DISABLED: "" });
    await options("pool options over the variables", POOL, { maxTransactionsInPool: 9, enabled: "true" }, { CORE_MAX_TRANSACTIONS_IN_POOL: "500", CORE_TRANSACTION_POOL_DISABLED: "1" });
    await options("pool options as text", POOL, {
        maxTransactionAge: "100",
        maxTransactionBytes: "1e5",
        storage: "/var/lib/relay/pool.sqlite",
        allowedSenders: [sender],
        enabled: "FALSE",
    });
    await options("pool fees", POOL, { dynamicFees: { enabled: false, minFeePool: "0", minFeeBroadcast: 10, addonBytes: { transfer: "10", burn: 7 } } });
    for (const [name, opts, env] of [
        ["pool size from a bad variable", {}, { CORE_MAX_TRANSACTIONS_IN_POOL: "many" }],
        ["per request 0 from a variable", {}, { CORE_TRANSACTION_POOL_MAX_PER_REQUEST: "0" }],
        ["age 0", { maxTransactionAge: 0 }],
        ["fees not an object", { dynamicFees: 5 }],
        ["a negative fee", { dynamicFees: { minFeePool: -1 } }],
        ["added bytes as a word", { dynamicFees: { addonBytes: { vote: "x" } } }],
        ["added bytes not an object", { dynamicFees: { addonBytes: [] } }],
        ["senders not a list", { allowedSenders: sender }],
        ["no workers", { workerPool: { workerCount: 0 } }],
        ["enabled as a word", { enabled: "yes" }],
        ["storage empty", { storage: "" }],
    ]) {
        await options(`pool refused: ${name}`, POOL, opts, env);
    }

    // ---- api ---------------------------------------------------------------------------------------
    await options("api defaults", API, {});
    await options("api from the variables", API, {}, {
        CORE_API_HOST: "127.0.0.1",
        CORE_API_PORT: "6003",
        CORE_API_DISABLED: "1",
        CORE_API_RATE_LIMIT_DISABLED: "1",
        CORE_API_RATE_LIMIT_USER_LIMIT: "500",
        CORE_API_RATE_LIMIT_USER_EXPIRES: "30",
        CORE_API_RATE_LIMIT_WHITELIST: "192.0.2.1,192.0.2.2",
        CORE_API_RATE_LIMIT_BLACKLIST: "198.51.100.1",
        CORE_API_SEMAPHORE_DISABLED: "1",
        CORE_API_TRUST_PROXY: "1",
        CORE_API_ESTIMATED_TOTAL_COUNT: "1",
    });
    await options("api HTTPS asked for", API, {}, { CORE_API_SSL: "1", CORE_API_SSL_KEY: "/etc/relay/key.pem", CORE_API_SSL_CERT: "/etc/relay/cert.pem" });
    await options("api options as text", API, {
        server: { http: { port: "7003", enabled: "false" } },
        plugins: {
            socketTimeout: "2000",
            whitelist: ["127.0.0.1"],
            trustProxy: "true",
            rateLimit: { points: "50", duration: "10", whitelist: ["192.0.2.9"] },
            semaphore: { database: { levelOne: { concurrency: "2", queueLimit: 4, maxOffset: "50" } } },
        },
        options: { estimateTotalCount: true },
    });
    for (const [name, opts, env] of [
        ["port from a bad variable", {}, { CORE_API_PORT: "http" }],
        ["port 0", { server: { http: { port: 0 } } }],
        ["points as a word", { plugins: { rateLimit: { points: "many" } } }],
        ["enabled as a word", { server: { http: { enabled: "yes" } } }],
        ["a negative offset", { plugins: { semaphore: { memory: { levelOne: { maxOffset: -1 } } } } }],
        ["socket timeout as a word", { plugins: { socketTimeout: "soon" } }],
    ]) {
        await options(`api refused: ${name}`, API, opts, env);
    }

    // ---- database ----------------------------------------------------------------------------------
    await options("database defaults", DATABASE, {});
    await options("database options as text", DATABASE, { apiConnectionTimeout: "5000", connection: { logging: "true", synchronise: "FALSE" } });
    for (const [name, opts] of [
        ["timeout 0", { apiConnectionTimeout: 0 }],
        ["connection null", { connection: null }],
        ["extra not an object", { connection: { extra: [] } }],
        ["type empty", { connection: { type: "" } }],
        ["synchronise as a word", { connection: { synchronise: "no" } }],
    ]) {
        await options(`database refused: ${name}`, DATABASE, opts);
    }

    // ---- app.json ----------------------------------------------------------------------------------
    const text = (value) => JSON.stringify(value, null, 4);
    for (const role of ["relay", "forger", "core"]) await app(`generated app.json, ${role}`, text(appJson(RELAY)), role);
    await app("relay profile in another order", text(appJson([...RELAY].reverse())));
    await app("flags and services", text(appJson(RELAY, { relay: { plugins: RELAY, flags: ["x"], services: {} } })));
    await app("entry options kept as given", text(appJson([{ package: API, options: { server: { http: { port: "7003" } } } }, RELAY[1]])));
    await app("an empty relay profile", text(appJson([])));
    for (const [name, value] of [
        ["an unknown package", appJson([...RELAY, { package: "@solar-network/nope" }])],
        ["a package with no service provider", appJson([{ package: "@solar-network/crypto" }])],
        ["an entry without a package", appJson([{}])],
        ["an empty package", appJson([{ package: "" }])],
        ["no forger profile", { core: { plugins: [] }, relay: { plugins: RELAY } }],
        ["an unknown profile key", appJson(RELAY, { relay: { plugins: RELAY, other: 1 } })],
        ["plugins not a list", appJson({})],
        ["options a list", appJson([{ package: API, options: [] }])],
        ["an unknown entry key", appJson([{ package: API, enabled: true }])],
        ["flags not a list", appJson(RELAY, { relay: { plugins: RELAY, flags: "x" } })],
        ["a bad core profile", appJson(RELAY, { core: { plugins: {} } })],
    ]) {
        await app(`refused: ${name}`, text(value));
    }
    await app("refused: not JSON", "{");
    return out;
};
