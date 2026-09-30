"use strict";
// Oracle operations of the listings over the state, run on the reference's own server modules
// (packages/api/dist: routes, controllers, DelegateSearchService and WalletSearchService) with the
// kernel's own StandardCriteriaService and PaginationService, over the reference's wallet
// repository as lib/state.js loads a fixture into it:
//   api.delegatesPage     {fixture, query, path?, orders?, extras?}
//   api.walletsPage       {fixture, query, path?, orders?, extras?}
//       -> {status: 200, body}, the response of GET path?query as hapi serialises it (BigNumber
//          and Semver values as text), {status: 404, message} for a delegate that is not found, or
//          {status} for any other refusal (422 for the route's validation, 500 for a throw)
//   api.delegateResource  {wallet, supply}  -> {resource} or {status: 500}
//
// path is the route under /api: /delegates (default), /delegates/{id}, /delegates/{id}/voters for
// the first; /wallets (default) and /wallets/top for the second.
//
// A fixture is the wallet fixture of tools/difftest's fixture module. `orders` sets the order of
// the address, username and public-key indexes as permutations of fixture positions (the order
// the reference's own indexing gives is the fixture's own); `extras` gives, by delegate name, the
// attributes a fixture has no way to state: rank, missed (blocks missed), productivity (in
// hundredths of a percent) and version. Both are what the listings read from the state. A
// delegate's attribute keys are put in the order a state built from the store gives them, and a
// delegate's second key after its delegate attribute (a wallet's attribute order follows its
// history, which a fixture does not state).
//
// api.delegateResource builds one delegate wallet from `wallet` (keys a, pk optional, u, vb, voters,
// rk, rs, pb, ff, fr, bf, dn, lb, missed, productivity, version; the amounts are decimal
// strings) and runs getDelegateResourceFromWallet at the decimal string `supply`.
// Loaded by lib/ops.js.

const path = require("path");

const { FixtureState } = require("../state");

// The keys of a delegate's attribute in the order of a wallet that the reference built from its
// store (a registration, then the round's rank, the vote balances' voters, the blocks, the
// version, the resignation, and the productivity the API adds), which a fixture's own loading does
// not give: the rank and the extras are set after it.
const DELEGATE_KEYS = [
    "username", "voteBalance", "forgedFees", "burnedFees", "forgedRewards", "donations", "producedBlocks", "rank", "voters",
    "lastBlock", "version", "resigned", "missedBlocks", "productivity",
];

module.exports = function apiStateOps({ solar, withConfig }) {
    const apiDir = path.join(solar.solarDir, "packages", "api");
    const api = (...parts) => require(path.join(apiDir, "dist", ...parts));
    const dependency = (name) => require(require.resolve(name, { paths: [apiDir] }));
    const kernel = () => require(path.join(solar.solarDir, "packages", "kernel", "dist"));
    const { BigNumber } = solar.lib.Utils;

    let fixtureState;
    const current = { repository: undefined };
    let server;

    const fixtures = () => {
        if (!fixtureState) fixtureState = new FixtureState(solar);
        return fixtureState;
    };

    /** The two search services and the controllers of the routes, over `current.repository`. */
    function controllers() {
        const { Services } = kernel();
        const services = () => {
            const service = (Class) => {
                const made = Object.create(Class.prototype);
                Object.defineProperty(made, "walletRepository", { get: () => current.repository });
                made.standardCriteriaService = new Services.Search.StandardCriteriaService();
                made.paginationService = new Services.Search.PaginationService();
                return made;
            };
            return { delegates: service(api("services").DelegateSearchService), wallets: service(api("services").WalletSearchService) };
        };
        const { delegates, wallets } = services();
        const make = (Class) => {
            const controller = Object.create(Class.prototype);
            controller.delegateSearchService = delegates;
            controller.walletSearchService = wallets;
            return controller;
        };
        return { delegates, wallets, make };
    }

    async function listingServer() {
        if (server) return server;
        const Hapi = dependency("@hapi/hapi");
        const Boom = dependency("@hapi/boom");
        const { make } = controllers();
        const wanted = new Map([
            [api("controllers", "delegates").DelegatesController, make(api("controllers", "delegates").DelegatesController)],
            [api("controllers", "wallets").WalletsController, make(api("controllers", "wallets").WalletsController)],
        ]);
        const unbuilt = new Proxy({}, { get: () => () => { throw new Error("not built"); } });
        const failAction = async (request, h, err) => Boom.badData(err.message);
        server = new Hapi.Server({ router: { stripTrailingSlash: true }, routes: { payload: { failAction }, validate: { failAction } } });
        server.app.app = {
            resolve: (Class) => wanted.get(Class) ?? unbuilt,
            getTagged: () => ({ get: () => 40, getRequired: () => 40, getOptional: (key, fallback) => fallback }),
        };
        server.app.schemas = api("schemas");
        const plugins = [api("plugins", "comma-array-query").commaArrayQuery, api("plugins", "dot-separated-query").dotSeparatedQuery];
        await server.register(plugins.map((plugin) => ({ plugin })));
        await server.register({ plugin: api("plugins", "pagination").pagination });
        await server.register({ plugin: api("handlers"), routes: { prefix: "/api" } });
        return server;
    }

    /** The positions of `order` as a permutation of `wanted`, or a refusal. */
    function permutation(name, order, wanted) {
        if (!Array.isArray(order) || order.length !== wanted.length || [...order].sort((a, b) => a - b).join() !== [...wanted].sort((a, b) => a - b).join()) {
            throw new TypeError(`orders.${name} is not a permutation of ${wanted.join(",")}`);
        }
        return order;
    }

    /** Load `fixture`, put the indexes in the orders of `orders` and set the delegates' extras. */
    async function load(fixture, orders = {}, extras = {}, height) {
        const state = fixtures();
        await state.load(fixture, height);
        const repository = state.chain.walletRepository;
        const { Contracts } = kernel();
        const wallets = fixture.map((w) => repository.findByAddress(w.a));
        const index = (name) => repository.getIndex(Contracts.State.WalletIndexes[name]);
        const rebuild = (name, positions, key, order) => {
            const walletIndex = index(name);
            if (walletIndex.keys().length !== positions.length) throw new Error(`the ${name} index holds ${walletIndex.keys().length} wallets, not ${positions.length}`);
            walletIndex.clear();
            for (const at of order === undefined ? positions : permutation(name, order, positions)) walletIndex.set(key(wallets[at]), wallets[at]);
        };
        const all = fixture.map((w, at) => at);
        rebuild("Addresses", all, (wallet) => wallet.getAddress(), orders.address);
        rebuild("Usernames", all.filter((at) => fixture[at].u !== undefined), (wallet) => wallet.getAttribute("delegate.username"), orders.username);
        rebuild("PublicKeys", all.filter((at) => fixture[at].pk !== undefined), (wallet) => wallet.getPublicKey(), orders.publicKey);
        for (const [username, extra] of Object.entries(extras)) {
            const delegate = repository.findByUsername(username).getAttribute("delegate");
            if (extra.rank !== undefined) delegate.rank = extra.rank;
            if (extra.missed !== undefined) delegate.missedBlocks = extra.missed;
            if (extra.productivity !== undefined) delegate.productivity = extra.productivity / 100;
            if (extra.version !== undefined) delegate.version = { version: extra.version };
        }
        for (const wallet of wallets) {
            const delegate = wallet.hasAttribute("delegate") ? wallet.getAttribute("delegate") : undefined;
            if (delegate === undefined) continue;
            const ordered = {};
            for (const name of DELEGATE_KEYS) if (name in delegate) ordered[name] = delegate[name];
            wallet.setAttribute("delegate", { ...ordered, ...delegate });
            if (wallet.hasAttribute("secondPublicKey")) {
                const second = wallet.getAttribute("secondPublicKey");
                wallet.forgetAttribute("secondPublicKey");
                wallet.setAttribute("secondPublicKey", second);
            }
        }
        return repository;
    }

    async function inject(url) {
        const s = await listingServer();
        const response = await s.inject({ method: "GET", url });
        if (response.statusCode === 200) return { status: 200, body: JSON.parse(response.payload) };
        if (response.statusCode === 404) return { status: 404, message: JSON.parse(response.payload).message };
        return { status: response.statusCode };
    }

    const listing = (defaultPath) => async ({ fixture, query, path: route = defaultPath, orders, extras }, height) => {
        if (typeof query !== "string") throw new TypeError("query must be text");
        if (typeof route !== "string" || !route.startsWith("/")) throw new TypeError("path must start with /");
        return withConfig(undefined, async () => {
            current.repository = await load(fixture, orders, extras, height);
            return inject(`/api${route}${query ? `?${query}` : ""}`);
        });
    };

    async function delegateResource({ wallet: w, supply }) {
        const { Wallets } = require(path.join(solar.solarDir, "packages", "state", "dist"));
        const { AttributeMap, AttributeSet } = kernel().Services.Attributes;
        const attributes = new AttributeSet();
        for (const key of ["votes", "delegate", "delegate.username", "delegate.voteBalance", "delegate.voters", "delegate.rank", "delegate.resigned"]) attributes.set(key);
        const wallet = new Wallets.Wallet(w.a, new AttributeMap(attributes));
        if (w.pk !== undefined) wallet.setPublicKey(w.pk);
        const big = (text) => BigNumber.make(text ?? "0");
        const delegate = {
            username: w.u,
            voteBalance: big(w.vb),
            voters: w.voters ?? 0,
            forgedFees: big(w.ff),
            burnedFees: big(w.bf),
            forgedRewards: big(w.fr),
            donations: big(w.dn),
            producedBlocks: w.pb ?? 0,
        };
        if (w.rk !== undefined) delegate.rank = w.rk;
        if (w.lb !== undefined) delegate.lastBlock = w.lb;
        if (w.missed !== undefined) delegate.missedBlocks = w.missed;
        if (w.productivity !== undefined) delegate.productivity = w.productivity / 100;
        if (w.version !== undefined) delegate.version = { version: w.version };
        wallet.setAttribute("delegate", delegate);
        if (w.rs !== undefined) delegate.resigned = w.rs;
        const service = Object.create(api("services").DelegateSearchService.prototype);
        try {
            return { resource: JSON.parse(JSON.stringify(service.getDelegateResourceFromWallet(wallet, supply))) };
        } catch {
            return { status: 500 };
        }
    }

    return {
        "api.delegatesPage": listing("/delegates"),
        "api.walletsPage": listing("/wallets"),
        "api.delegateResource": delegateResource,
    };
};
