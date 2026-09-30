"use strict";
// Oracle operations of the wallet search and its collation:
//   api.walletsLike     {fixture, ranks?, input, milestones?} -> {wallets}
//                       the reference's getWalletsLike (services/wallet-search-service.ts) over the
//                       wallets of a fixture, in the fixture's order (the address index's order),
//                       as the route serialises them; `ranks` is [[address, rank], ...], the ranks
//                       the reference's round has set on delegates (a fixture has none)
//   js.localeCompare    {a, b} -> {order}  (a.localeCompare(b, "en", {numeric: true}), -1, 0 or 1)
// Loaded by lib/ops.js.

const path = require("path");

const { FixtureState } = require("../state");

// The height a fixture is loaded at: the search does not read it.
const LAST_BLOCK_HEIGHT = 100;

module.exports = function apiJsOps({ solar, withConfig }) {
    let fixtureState;
    let WalletSearchService;

    const service = (repository) => {
        if (!WalletSearchService) {
            ({ WalletSearchService } = require(path.join(solar.solarDir, "packages", "api", "dist", "services", "wallet-search-service")));
        }
        // The service holds its repository as an injected property; a bare instance takes it directly.
        const instance = Object.create(WalletSearchService.prototype);
        instance.walletRepository = repository;
        return instance;
    };

    return {
        "api.walletsLike": async ({ fixture, ranks = [], input, milestones }) => {
            if (typeof input !== "string") throw new TypeError("input must be text");
            return withConfig(milestones, async () => {
                if (!fixtureState) fixtureState = new FixtureState(solar);
                await fixtureState.load(fixture, LAST_BLOCK_HEIGHT);
                const repository = fixtureState.chain.walletRepository;
                for (const [address, rank] of ranks) repository.findByAddress(address).setAttribute("delegate.rank", rank);
                return { wallets: JSON.parse(JSON.stringify(service(repository).getWalletsLike(input))) };
            });
        },
        "js.localeCompare": async ({ a, b }) => {
            if (typeof a !== "string" || typeof b !== "string") throw new TypeError("a and b must be text");
            return { order: a.localeCompare(b, "en", { numeric: true }) };
        },
    };
};
