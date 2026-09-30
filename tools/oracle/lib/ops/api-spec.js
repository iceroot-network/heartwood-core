"use strict";
// Oracle operations of the chain routes' documents and of the query fuzz target's replay:
//   api.spec                {network, pool, blockchain, basePath, version} -> {document}
//                           the reference's www/api.json as its server patches it at boot
//                           (server.ts boot): the base path in `servers`, the version, the
//                           network character in the address patterns and the example addresses,
//                           the pool's maxTransactionsPerRequest, the registered types, groups
//                           and versions, and the missed-block summaries' days; then cut to what
//                           Heartwood serves (the registered handlers of the six kept types, the
//                           three /locks paths taken out).
//                           network is {pubKeyHash} (the address version byte), pool is
//                           {maxTransactionsPerRequest}, blockchain is {missedBlocksLookback} in
//                           seconds
//   api.transactionSchemas  {network} -> {data}  (GET /transactions/schemas: the schema properties
//                           of the activated handlers of the kept types, by type group and type)
//   api.feeStatistics       {rows, days?} -> {meta, data}  (GET /node/fees over the rows the
//                           repository returns: {typeGroup, type, avg, burned, max, min, sum},
//                           the numbers as text)
//   api.fuzzQuery           {hex} -> {status, query?, page?}  (a fuzz input of the api_query
//                           target: the route's choice byte, then the raw query, which goes as
//                           request.query through the reference's query plugins, the route's
//                           validation and the pagination plugin; {} for an empty input)
// Loaded by lib/ops.js.

const fs = require("fs");
const path = require("path");
const Querystring = require("querystring");

// The types of the handlers Heartwood carries, as "typeGroup/type".
const KEPT = new Set(["1/1", "1/2", "1/6", "1/7", "2/0", "2/2"]);

// The paths of the removed lock (HTLC) operations.
const REMOVED_PATHS = ["/locks", "/locks/{id}", "/locks/unlocked"];

// The routes of the query target, in the order of its choice byte: the path a request has (a
// valid one, since only the query is fuzzed) and whether the route lists a page.
const ROUTES = [
    ["/api/transactions", true],
    ["/api/blocks/12/transactions", true],
    ["/api/wallets/genesis_1/transactions", true],
    ["/api/peers", true],
    ["/api/transactions/unconfirmed", true],
    ["/api/blocks/12", false],
    ["/api/node/status", false],
    ["/api/blockchain", false],
    ["/api/blockchain/search/abc", false],
    ["/api/node/fees", false],
    ["/api/transactions/schemas", false],
];

module.exports = function apiSpecOps({ solar, devnet, cm, atHeight, handlerRegistry, plain }) {
    const apiDir = path.join(solar.solarDir, "packages", "api");
    const api = (...parts) => require(path.join(apiDir, "dist", ...parts));
    const dependency = (name) => require(require.resolve(name, { paths: [apiDir] }));

    const isKept = (handler) => {
        const constructor = handler.getConstructor();
        return KEPT.has(`${constructor.typeGroup}/${constructor.type}`);
    };

    /** Run fn under the devnet network with the address version byte of `network`. */
    async function underNetwork(network, fn) {
        const hash = network && network.pubKeyHash;
        if (!Number.isInteger(hash) || hash < 0 || hash > 255) throw new TypeError("network.pubKeyHash must be a byte");
        const config = JSON.parse(JSON.stringify(devnet.config));
        config.network.pubKeyHash = hash;
        // The donation addresses of the second milestone are of the devnet's version byte.
        config.milestones = config.milestones.slice(0, 1);
        try {
            cm.setConfig(config);
            return await fn();
        } finally {
            devnet.restore();
        }
    }

    const count = (value, name) => {
        if (!Number.isSafeInteger(value)) throw new TypeError(`${name} must be an integer`);
        return value;
    };

    function spec({ network, pool, blockchain, basePath, version }, height) {
        if (typeof basePath !== "string" || typeof version !== "string") throw new TypeError("basePath and version must be text");
        const maxTransactions = count(pool && pool.maxTransactionsPerRequest, "pool.maxTransactionsPerRequest");
        const seconds = count(blockchain && blockchain.missedBlocksLookback, "blockchain.missedBlocksLookback");
        return underNetwork(network, () =>
            atHeight(height, () => {
                // The reference's boot(), on its own document.
                const swaggerJson = JSON.parse(fs.readFileSync(path.join(apiDir, "dist", "www", "api.json"), "utf8"));
                const dummyAddress = solar.lib.Identities.Address.fromPassphrase("");
                const networkCharacter = dummyAddress.slice(0, 1);
                swaggerJson.servers.push({ url: basePath });
                swaggerJson.info.version = version;
                swaggerJson.components.schemas.address.pattern = swaggerJson.components.schemas.recipientId.pattern =
                    swaggerJson.components.schemas.address.pattern.substring(0, 1) +
                    networkCharacter +
                    swaggerJson.components.schemas.address.pattern.substring(2);
                swaggerJson.components.schemas.recipientId.example =
                    swaggerJson.components.schemas.transaction.properties.asset.example.transfers[0].recipientId = dummyAddress;
                swaggerJson.components.schemas.walletIdentifier.pattern =
                    swaggerJson.components.schemas.walletIdentifier.pattern.substring(0, 1) +
                    networkCharacter +
                    swaggerJson.components.schemas.walletIdentifier.pattern.substring(2);
                swaggerJson.components.schemas.transactions.properties.transactions.maxItems = maxTransactions;

                const typeGroups = new Set();
                const types = new Set();
                for (const handler of handlerRegistry().getRegisteredHandlers().filter(isKept)) {
                    const constructor = handler.getConstructor();
                    typeGroups.add(constructor.typeGroup);
                    types.add(constructor.type);
                }
                swaggerJson.components.schemas.transactionTypes.enum = [...types];
                swaggerJson.components.schemas.transactionTypeGroups.enum = [...typeGroups];
                swaggerJson.components.schemas.transactionVersions.enum = solar.lib.Transactions.schemas.transactionBaseSchema.properties.version.enum;

                const days = +(seconds / 86400).toFixed(2);
                for (const route of ["/blocks/missed", "/delegates/{identifier}/blocks/missed"]) {
                    swaggerJson.paths[route].get.summary = swaggerJson.paths[route].get.summary.replace("X", days);
                }
                for (const removed of REMOVED_PATHS) delete swaggerJson.paths[removed];
                return { document: plain(swaggerJson) };
            }),
        );
    }

    function transactionSchemas({ network }, height) {
        return underNetwork(network, () =>
            atHeight(height, async () => {
                const data = {};
                for (const handler of (await handlerRegistry().getActivatedHandlers()).filter(isKept)) {
                    const constructor = handler.getConstructor();
                    (data[constructor.typeGroup] ||= {})[constructor.type] = constructor.getSchema().properties;
                }
                return { data: plain(data) };
            }),
        );
    }

    async function feeStatistics({ rows, days }) {
        if (!Array.isArray(rows)) throw new TypeError("rows must be a list");
        if (days !== undefined && !Number.isSafeInteger(days)) throw new TypeError("days must be an integer");
        const { NodeController } = api("controllers", "node");
        const controller = Object.create(NodeController.prototype);
        controller.nullHandlerRegistry = handlerRegistry();
        controller.transactionRepository = { getFeeStatistics: async () => rows.map((row) => ({ ...row })) };
        return plain(await controller.fees({ query: { days } }, {}));
    }

    // One server for every api.fuzzQuery call. A request takes the raw query of the call as its
    // query (an extension that runs before the reference's query plugins), then goes through them,
    // the route's validation and the pagination plugin, to a controller that records the query it
    // is given and answers a page of `total` matches on the routes that list one.
    let server;
    const call = { raw: undefined, total: 0, paged: false, query: undefined };

    async function fuzzServer() {
        if (server) return server;
        const Hapi = dependency("@hapi/hapi");
        const Boom = dependency("@hapi/boom");
        const { Controller } = api("controllers", "controller");
        const answer = (request) => {
            call.query = request.query;
            if (!call.paged) return {};
            const page = Controller.prototype.getListingPage.call({}, request);
            const results = Math.max(0, Math.min(page.limit, call.total - page.offset));
            return { results: Array(results).fill(null), totalCount: call.total, meta: { totalCountIsEstimate: false } };
        };
        const controller = new Proxy({}, { get: () => answer });
        const failAction = async (request, h, err) => Boom.badData(err.message);
        server = new Hapi.Server({ router: { stripTrailingSlash: true }, routes: { payload: { failAction }, validate: { failAction } } });
        server.app.app = {
            resolve: () => controller,
            getTagged: () => ({ get: () => 40, getRequired: () => 40, getOptional: (key, fallback) => fallback }),
        };
        server.app.schemas = api("schemas");
        server.ext("onRequest", (request, h) => {
            for (const key of Object.keys(request.query)) delete request.query[key];
            for (const [key, value] of Object.entries(call.raw)) request.query[key] = value;
            return h.continue;
        });
        await server.register(
            [api("plugins", "comma-array-query").commaArrayQuery, api("plugins", "dot-separated-query").dotSeparatedQuery].map((plugin) => ({ plugin })),
        );
        await server.register({ plugin: api("plugins", "pagination").pagination });
        await server.register({ plugin: api("handlers"), routes: { prefix: "/api" } });
        return server;
    }

    async function fuzzQuery({ hex }) {
        if (typeof hex !== "string" || !/^([0-9a-f]{2})*$/.test(hex)) throw new TypeError("hex must be hexadecimal text");
        const bytes = Buffer.from(hex, "hex");
        if (bytes.length === 0) return {};
        const choice = bytes[0];
        const [requestPath, paged] = ROUTES[choice % ROUTES.length];
        const s = await fuzzServer();
        call.raw = Object.assign(Object.create(null), Querystring.parse(bytes.subarray(1).toString("utf8")));
        call.total = choice * 37;
        call.paged = paged;
        call.query = undefined;
        const response = await s.inject({ method: "GET", url: requestPath });
        if (response.statusCode !== 200) return { status: response.statusCode };
        const out = { status: 200, query: JSON.parse(JSON.stringify(call.query)) };
        if (paged) out.page = JSON.parse(response.payload).meta;
        return out;
    }

    return {
        "api.spec": spec,
        "api.transactionSchemas": transactionSchemas,
        "api.feeStatistics": feeStatistics,
        "api.fuzzQuery": fuzzQuery,
    };
};
