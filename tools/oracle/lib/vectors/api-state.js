"use strict";
// The records of kinds api.delegatesPage, api.walletsPage and api.delegateResource in V34: the
// listings over the state (GET /delegates, /delegates/{id}, /delegates/{id}/voters, /wallets and
// /wallets/top) on wallet fixtures, and one delegate's resource at a stated supply.
//
// The fixtures are a roster of twelve delegates (equal and unequal counts and amounts, resigned
// and unranked ones, some without a version or a last block) with voters, a wallet without a key,
// one with a second key and wallets of equal balances, whose supply is 8,000 so that two delegates
// hold 1.125 and 0.625 percent of it (the ties of toFixed(2)); a roster at the smallest supply
// and one with balances near the top of the amounts. A listing runs on the fixture's own index
// orders and on others (reversed, rotated, shuffled) to show where a tie leaves the order to the
// index. Coverage is checked as the records are made: a criterion of every kind, the refusals
// (a range on a text value is a 500, a % in a username a 422), pages, and each route. A last
// fixture has delegates with second keys, one set before its delegate record (`so`) and one
// after, for the attribute order and the `attributes.*` wildcard criteria.

const HEIGHT = 1000;
const SUPPLY = 8000;

module.exports = async function apiStateVectors({ solar, rec, must }) {
    const { Identities } = solar.lib;
    const out = [];
    const seen = { statuses: new Set() };

    const key = (passphrase) => Identities.Keys.fromPassphrase(passphrase).publicKey.secp256k1;
    const address = (passphrase) => Identities.Address.fromPublicKey(key(passphrase), 90);
    const hex = (i) => (i + 16).toString(16).padStart(2, "0").repeat(32);

    // The roster: delegates 0 to 11, voters, the wallets of one balance, the rest.
    const PB = [0, 5, 5, 12, 12, 12, 30, 7, 1, 100, 40, 3];
    const FF = ["0", "300", "300", "1500", "20", "20", "999999999999", "5", "5", "7", "1234567890123", "0"];
    const FR = ["0", "600", "600", "3000", "40", "0", "1999999999999", "10", "10", "14", "2469135780246", "1"];
    const BF = ["0", "270", "270", "1350", "18", "18", "899999999999", "4", "4", "6", "1111111101110", "0"];
    const DN = ["0", "60", "0", "300", "4", "4", "199999999999", "1", "1", "1", "246913578024", "0"];
    const name = (i) => `v34d${i}`;
    const wallet = (passphrase, extra = {}) => ({ a: address(passphrase), pk: key(passphrase), bal: "100", n: "1", ...extra });
    const delegate = (i) => {
        const extra = { u: name(i), pb: PB[i], ff: FF[i], fr: FR[i], bf: BF[i], dn: DN[i] };
        if (i % 3 !== 0) extra.lb = hex(i);
        if (i === 9) Object.assign(extra, { rs: 1, resignedAt: 500 });
        if (i === 10) Object.assign(extra, { rs: 0, resignedAt: 800 });
        return wallet(`v34 delegate ${i}`, { ...extra, bal: i === 11 ? "0" : "100" });
    };
    const roster = () => [
        ...Array.from({ length: 12 }, (_, i) => delegate(i)),
        wallet("v34 voter 0", { bal: "90", v: [[name(0), 10000]] }),
        wallet("v34 voter 1", { bal: "50", v: [[name(1), 10000]] }),
        wallet("v34 voter 2", { bal: "1000", n: "7", v: [[name(2), 5000], [name(3), 5000]] }),
        wallet("v34 voter 3", { bal: "333", v: [[name(2), 3333], [name(4), 3333], [name(5), 3334]] }),
        wallet("v34 voter 4", { bal: "7", n: "0", v: [[name(6), 10000]] }),
        wallet("v34 voter 5", { bal: "4000", n: "12", v: [[name(7), 10000]] }),
        { a: address("v34 no key"), bal: "250", n: "0" },
        wallet("v34 second key", { bal: "100", spk: key("v34 second secret") }),
        wallet("v34 voter 8", { bal: "0", n: "3", v: [[name(8), 10000]] }),
        wallet("v34 voter 9", { bal: "10", v: [[name(9), 10000]] }),
        ...Array.from({ length: 6 }, (_, i) => (i % 2 === 0 ? wallet(`v34 equal ${i}`) : { a: address(`v34 equal ${i}`), bal: "100", n: "0" })),
        wallet("v34 filler", { bal: "460", n: "2" }),
    ];
    const DELEGATES = 12;
    const EXTRAS = {
        [name(0)]: { rank: 1, missed: 0, productivity: 10000, version: "4.3.0" },
        [name(1)]: { rank: 2, missed: 3, productivity: 9950, version: "4.3.1" },
        [name(2)]: { rank: 3, missed: 3, productivity: 9950, version: "4.10.0" },
        [name(3)]: { rank: 4, missed: 1, productivity: 9999, version: "5.0.0-next.1" },
        [name(4)]: { rank: 5, missed: 40, productivity: 7500 },
        [name(5)]: { rank: 6, missed: 0, productivity: 10000, version: "4.3.0" },
        [name(6)]: { rank: 7, missed: 12, productivity: 9123 },
        [name(7)]: { rank: 8, version: "1.0.0" },
        [name(8)]: { rank: 9 },
        [name(11)]: { rank: 10, missed: 2, productivity: 5 },
    };
    const A = (i) => address(`v34 delegate ${i}`);
    const K = (i) => key(`v34 delegate ${i}`);
    const V = (i) => address(`v34 voter ${i}`);

    // The orders a listing runs the fixture's indexes in: the fixture's own (none given), the
    // reverse, a rotation and a fixed shuffle of the positions each index holds.
    const positions = (fixture, has) => fixture.map((w, at) => at).filter((at) => has(fixture[at]));
    const shuffled = (list, seed) => {
        const copy = [...list];
        let state = seed;
        for (let i = copy.length - 1; i > 0; i--) {
            state = (state * 1103515245 + 12345) % 2147483648;
            const j = state % (i + 1);
            [copy[i], copy[j]] = [copy[j], copy[i]];
        }
        return copy;
    };
    const orders = (fixture, kind) => {
        const indexes = { address: () => true, username: (w) => w.u !== undefined, publicKey: (w) => w.pk !== undefined };
        const made = {};
        for (const [index, has] of Object.entries(indexes)) {
            const list = positions(fixture, has);
            made[index] = kind === "reverse" ? [...list].reverse() : kind === "rotate" ? [...list.slice(5), ...list.slice(0, 5)] : shuffled(list, 7);
        }
        return made;
    };

    const add = async (op, label, input) => {
        const record = must(await rec(op, HEIGHT, JSON.parse(JSON.stringify(input))));
        const status = record.output.status;
        seen.statuses.add(`${op}:${status}`);
        out.push({ ...record, name: `${op}: ${label}` });
    };
    const list = async (op, fixture, queries, options = {}) => {
        for (const query of queries) {
            const input = { fixture, query, ...(options.path ? { path: options.path } : {}), ...(options.orders ? { orders: options.orders } : {}), ...(options.extras ? { extras: options.extras } : {}) };
            await add(op, `${options.path ?? ""}?${query}${options.orders ? " (index orders)" : ""}`, input);
        }
    };

    const F = roster();
    const pkPrefix = K(3).slice(0, 4);
    const delegateQueries = [
        "",
        // username, address, publicKey
        `username=${name(3)}`, `username=${name(3)},${name(5)},nosuch`, `username=${name(3)}&username=${name(4)}`, "username=nosuch",
        "username=%25d1", "username=v%25", "username=A", "username=", `username=${"a".repeat(21)}`,
        `address=${A(2)}`, `address=${A(2)},${A(4)}`, `address=${A(2).slice(0, 5)}%25`, `address=%25${A(2).slice(-3)}`, "address=%25%25%25",
        `publicKey=${K(2)}`, `publicKey=${pkPrefix}%25`, `publicKey=%25${K(4).slice(-4)}`, "publicKey=zz",
        // votesReceived
        "votesReceived.percent=1.125", "votesReceived.percent=0.63", "votesReceived.percent.from=0.5&votesReceived.percent.to=1.2",
        "votesReceived.percent.from=1", "votesReceived.percent.to=0.625", "votesReceived.percent=101", "votesReceived.percent=-1",
        "votesReceived.votes=90", "votesReceived.votes.from=100", "votesReceived.votes.from=90&votesReceived.votes.to=500", "votesReceived.votes=-5",
        "votesReceived.voters=1", "votesReceived.voters.from=1&votesReceived.voters.to=2", "votesReceived.voters=x",
        // rank and resignation
        "rank=1", "rank=1,3,5", "rank.from=2&rank.to=4", "rank.from=9", "rank=0", "rank=1.5",
        "isResigned=true", "isResigned=false", "isResigned=true&isResigned=false", "isResigned=maybe",
        "resignationType=permanent", "resignationType=temporary", "resignationType=other",
        // blocks
        "blocks.produced=12", "blocks.produced.from=5&blocks.produced.to=12", "blocks.produced.from=101", "blocks.missed=3", "blocks.missed.from=1",
        "blocks.productivity.from=99.5", "blocks.productivity=100", "blocks.productivity.to=76.5", `blocks.last.id=${hex(4)}`, "blocks.last.height.from=1", "blocks.last.timestamp.unix=5",
        // forged
        "forged.fees=300", "forged.fees.from=300&forged.fees.to=1500", "forged.total.from=1", "forged.total=0", "forged.burnedFees.from=1000",
        "forged.rewards=600", "forged.donations.from=1", "forged.fees=-1",
        // version
        "version=4.3.0", "version=4.3.0,5.0.0-next.1", "version.from=4.3.0&version.to=4.10.0", "version.from=4.3.1", "version=>=4.3.0", "version=x.y",
        // several criteria
        `isResigned=false&blocks.produced.from=5&orderBy=blocks.produced:desc`, `username=${name(1)}&rank=2`, `username=${name(1)}&rank=3`,
        "nosuchkey=1", "attributes.a=1",
        // orderBy, on flat, nested and missing values
        "orderBy=username:desc", "orderBy=blocks.produced:desc", "orderBy=blocks.produced:desc,forged.fees:asc", "orderBy=forged.total:desc", "orderBy=forged.fees",
        "orderBy=votesReceived.percent:desc", "orderBy=votesReceived.votes:asc", "orderBy=votesReceived.voters:desc", "orderBy=version:desc", "orderBy=version:asc",
        "orderBy=blocks.last.height:desc", "orderBy=blocks.last.id", "orderBy=rank:desc", "orderBy=isResigned:desc,username:desc", "orderBy=blocks.productivity:asc",
        "orderBy=blocks.missed:desc", "orderBy=resignationType:asc", "orderBy=publicKey:desc", "orderBy=address", "orderBy=blocks.last", "orderBy=nosuch", "orderBy=username:up", "orderBy=",
        // pages
        "limit=3", "limit=3&page=2", "limit=5&page=3", "page=2&limit=5&orderBy=username:desc", "offset=4&limit=3", "page=99", "limit=0", "limit=101", "page=0", "limit=100",
    ];
    await list("api.delegatesPage", F, delegateQueries, { extras: EXTRAS });

    // No extras at all: no rank, no version, no productivity, in the fixture's own order.
    await list("api.delegatesPage", F, ["", "orderBy=rank:desc", "orderBy=version:asc", "version=4.3.0", "rank=1"]);

    // Ties in each index order.
    const tied = ["orderBy=blocks.produced:desc", "orderBy=forged.fees:asc&limit=5", "orderBy=votesReceived.percent:desc&limit=4", "orderBy=blocks.missed:asc", "orderBy=version:desc", "orderBy=rank:asc"];
    for (const kind of ["reverse", "rotate", "shuffle"]) await list("api.delegatesPage", F, tied, { extras: EXTRAS, orders: orders(F, kind) });

    // The delegate routes.
    const routes = [
        `/delegates/${name(3)}`, `/delegates/${A(3)}`, `/delegates/${K(3)}`, `/delegates/${name(9)}`, `/delegates/${name(10)}`, `/delegates/${name(11)}`,
        "/delegates/nosuch", `/delegates/${A(0).replace(/.$/, A(0).endsWith("a") ? "b" : "a")}`, `/delegates/${V(0)}`, `/delegates/${key("v34 voter 0")}`, "/delegates/UPPER", "/delegates/%25",
    ];
    for (const path of routes) await add("api.delegatesPage", `${path}`, { fixture: F, query: "", path, extras: EXTRAS });
    await add("api.delegatesPage", "a delegate with a version", { fixture: F, query: "", path: `/delegates/${name(2)}`, extras: EXTRAS });
    await add("api.delegatesPage", "a delegate without one", { fixture: F, query: "", path: `/delegates/${name(4)}` });

    // The voters.
    const voters = [
        "", "orderBy=balance:asc", "balance.from=100", "balance.to=100&orderBy=address:desc", "limit=1&page=2", `address=${V(2)}`, `publicKey=${key("v34 voter 2")}`,
        "attributes.secondPublicKey=1", "nonce.from=1", "votingFor=x", "orderBy=nonce:desc", "orderBy=attributes.delegate.username", "publicKey=02%25", "orderBy=publicKey:desc,address:asc",
    ];
    for (const of of [2, 4, 9]) await list("api.delegatesPage", F, voters.slice(0, 4), { path: `/delegates/${name(of)}/voters`, extras: EXTRAS });
    await list("api.delegatesPage", F, voters.slice(4), { path: `/delegates/${name(2)}/voters` });
    await list("api.delegatesPage", F, ["", "orderBy=balance:asc"], { path: `/delegates/${A(2)}/voters` });
    await list("api.delegatesPage", F, [""], { path: `/delegates/${K(2)}/voters` });
    await list("api.delegatesPage", F, ["", "limit=2"], { path: `/delegates/${name(2)}/voters`, orders: orders(F, "shuffle") });
    await list("api.delegatesPage", F, [""], { path: `/delegates/${name(0)}/voters`, orders: orders(F, "reverse") });
    await list("api.delegatesPage", F, ["", "limit=2"], { path: "/delegates/nosuch/voters" });
    await list("api.delegatesPage", F, [""], { path: `/delegates/${V(0)}/voters` });
    await list("api.delegatesPage", F, [""], { path: `/delegates/${name(11)}/voters` });

    // The smallest and the largest supplies.
    const one = [{ ...delegate(1), bal: "0", pb: 0 }, { ...wallet("v34 lone voter"), bal: "1", v: [[name(1), 10000]] }];
    await list("api.delegatesPage", one, ["", "votesReceived.percent=100", "orderBy=votesReceived.percent:desc"]);
    await add("api.delegatesPage", "a delegate with no supply behind it", { fixture: [{ ...delegate(1), bal: "0" }], query: "" });
    const huge = [
        { ...delegate(1), bal: "12000000000000000000" },
        { ...delegate(2), bal: "1000000000000000000" },
        { ...wallet("v34 huge voter"), bal: "6000000000000000000", v: [[name(1), 10000]] },
        { ...wallet("v34 huge voter b"), bal: "3999999999999999999", v: [[name(2), 5000], [name(1), 5000]] },
    ];
    await list("api.delegatesPage", huge, ["", "orderBy=votesReceived.votes:desc", "votesReceived.votes.from=1000000000000000000", "votesReceived.percent.from=50"], { extras: { [name(1)]: { rank: 1 }, [name(2)]: { rank: 2 } } });

    // The wallets, at their own order and at others.
    const W = F;
    const wallets = [
        "", `address=${V(2)}`, `address=${V(2)},${V(3)},nosuch`, `address=${V(2).slice(0, 4)}%25`, `address=%25${V(2).slice(-3)}`, "address=zz",
        `publicKey=${key("v34 voter 2")}`, "publicKey=02%25", "publicKey=03%25", "balance=100", "balance=100,90", "balance.from=100", "balance.to=90", "balance.from=50&balance.to=500",
        "balance=-1", "balance.from=1&balance.to=x", "nonce=1", "nonce.from=2", "nonce=0", "nonce=-1",
        "attributes.delegate.username=" + name(3), "attributes.delegate.producedBlocks=12", "attributes.delegate.rank=3", "attributes.delegate.resigned=permanent", "attributes.delegate.version=4.3.0",
        "attributes.delegate.lastBlock=" + hex(4), "attributes.secondPublicKey=" + key("v34 second secret"), "attributes.secondPublicKey.from=1&attributes.secondPublicKey.to=2", "attributes.delegate.voteBalance=90",
        "attributes.delegate.username=%25d1", "attributes.delegate.username=%25", "attributes.delegate=x", "attributes.nosuch=1", "attributes.delegate.username=v34d3,v34d4",
        "attributes.delegate.producedBlocks.from=5&attributes.delegate.producedBlocks.to=12", "attributes.delegate.forgedFees.from=300", "attributes.delegate.username.from=a",
        "votingFor=x", `votingFor.${name(0)}.percent=100`,
        "orderBy=balance:asc", "orderBy=nonce:desc", "orderBy=address:desc", "orderBy=publicKey:asc", "orderBy=attributes.delegate.username", "orderBy=attributes.delegate.producedBlocks:desc",
        "orderBy=attributes.delegate.voteBalance:desc", "orderBy=attributes.secondPublicKey", "orderBy=attributes.delegate.version:desc", "orderBy=attributes.nosuch", "orderBy=attributes", "orderBy=nosuch",
        "orderBy=balance:desc,nonce:asc", "orderBy=balance:asc,publicKey:desc",
        "limit=5", "limit=5&page=2", "limit=10&page=3", "offset=7&limit=4", "page=99", "limit=0", "limit=101", "limit=100&page=1", "page=2&limit=5&balance.from=100",
    ];
    await list("api.walletsPage", W, wallets, { extras: EXTRAS });
    await list("api.walletsPage", W, ["", "limit=5", "limit=5&page=2", "orderBy=balance:asc", "balance.from=100", "orderBy=nonce:asc&limit=8"], { path: "/wallets/top", extras: EXTRAS });
    for (const kind of ["reverse", "rotate", "shuffle"]) {
        await list("api.walletsPage", W, ["", "limit=8", "balance=100", "orderBy=nonce:desc&limit=12", "orderBy=attributes.delegate.username&limit=15", "orderBy=publicKey"], { orders: orders(W, kind), extras: EXTRAS });
        await list("api.walletsPage", W, ["", "limit=6&page=2"], { path: "/wallets/top", orders: orders(W, kind), extras: EXTRAS });
    }
    await list("api.walletsPage", W, ["orderBy=publicKey", "publicKey=02%25&orderBy=address:desc"], { orders: { publicKey: orders(W, "shuffle").publicKey }, extras: EXTRAS });
    await list("api.walletsPage", [], ["", "limit=5"]);
    await list("api.walletsPage", [], [""], { path: "/wallets/top" });
    await list("api.walletsPage", huge, ["", "orderBy=balance:asc", "balance.from=6000000000000000000", "balance=3999999999999999999"]);

    // The attribute order: a delegate whose second key came first, one whose came after, and
    // delegates, voters and a wallet without a second key; the wildcard criteria test each value
    // of `attributes` in its order, and throw on a second key given an object.
    const second = (i) => key(`v34 second secret ${i}`);
    const ordered = [
        { ...delegate(1), spk: second(1), so: true },
        { ...delegate(2), spk: second(2) },
        delegate(3),
        delegate(4),
        wallet("v34 voter 0", { bal: "90", v: [[name(1), 10000]] }),
        wallet("v34 voter 1", { bal: "50", v: [[name(3), 10000]] }),
        wallet("v34 filler", { bal: "460", n: "2" }),
    ];
    const withoutSecond = ordered.filter((w) => w.spk === undefined || w.so === true);
    const wildcards = [
        "", "limit=2", "orderBy=balance:asc", `attributes.*.username=${name(1)}`, `attributes.*.username=${name(2)}`, `attributes.*.username=${name(3)}`,
        "attributes.*.producedBlocks=5", "attributes.*.producedBlocks.from=1", `attributes.*=${second(1)}`, `attributes.*=${second(2)}`, "attributes.*=x",
        `attributes.*.username=${name(1)},${name(3)}`, "attributes.*.nosuch=1", `attributes.secondPublicKey=${second(1)}`, "orderBy=attributes.secondPublicKey",
    ];
    await list("api.walletsPage", ordered, wildcards);
    await list("api.walletsPage", withoutSecond, wildcards);
    await list("api.walletsPage", ordered, ["", "limit=3"], { path: "/wallets/top" });
    await list("api.delegatesPage", ordered, ["", `attributes.*.username=${name(1)}`], { path: `/delegates/${name(1)}/voters` });
    await list("api.delegatesPage", ordered, ["", `username=${name(1)}`]);

    // The delegate resource at a stated supply.
    const base = { a: A(1), pk: K(1), u: name(1), vb: "90", voters: 1, pb: 5, ff: "300", fr: "600", bf: "270", dn: "60", rk: 2, lb: hex(1), missed: 3, productivity: 9950, version: "4.3.1" };
    const resource = async (label, changes, supply) => add("api.delegateResource", label, { wallet: { ...base, ...changes }, supply });
    await resource("1.125 percent, a tie of toFixed(2)", { vb: "9" }, "800");
    await resource("0.625 percent, a tie of toFixed(2)", { vb: "5" }, "800");
    await resource("the tie at 90 of 8,000", {}, "8000");
    await resource("the tie at 50 of 8,000", { vb: "50" }, "8000");
    await resource("2.675 percent", { vb: "107" }, "4000");
    await resource("1.005 percent", { vb: "201" }, "20000");
    await resource("a third", { vb: "1" }, "3");
    await resource("no votes", { vb: "0" }, "8000");
    await resource("everything", { vb: "8000" }, "8000");
    await resource("more than the supply", { vb: "9000" }, "8000");
    await resource("the smallest supply", { vb: "1" }, "1");
    await resource("no supply", { vb: "1" }, "0");
    await resource("no supply and no votes", { vb: "0" }, "0");
    await resource("a huge supply", { vb: "12345678901234567890" }, "1000000000000000000000000000000");
    await resource("a tiny share of a huge supply", { vb: "1" }, "1000000000000000000000000000000");
    await resource("a share beyond the arithmetic's four digits", { vb: "1" }, `1${"0".repeat(37)}`);
    // The exponents at which Math.pow(10, k) is off by one step in Node 18: 26, 29, 34, 36 and 39.
    for (const k of [26, 29, 34, 36, 39]) await resource(`a supply of ${k - 3} digits behind one vote`, { vb: "1" }, `1${"0".repeat(k - 4)}`);
    await resource("resigned for good", { rs: 1 }, "8000");
    await resource("resigned for a while", { rs: 0 }, "8000");
    await resource("no version", { version: undefined }, "8000");
    await resource("no rank, last block, missed blocks or productivity", { rk: undefined, lb: undefined, missed: undefined, productivity: undefined }, "8000");
    await resource("no missed blocks but a productivity", { missed: undefined }, "8000");
    await resource("a productivity of one hundredth", { productivity: 1 }, "8000");
    await resource("no public key", { pk: undefined }, "8000");
    await resource("forged total below zero", { ff: "1", fr: "1", bf: "100", dn: "100" }, "8000");
    await resource("forged amounts of 20 digits", { ff: "12345678901234567890", fr: "9876543210987654321", bf: "1", dn: "2" }, "8000");
    await resource("a long username", { u: "a".repeat(20) }, "8000");
    await resource("many voters and blocks", { voters: 4096, pb: 1000000 }, "8000");

    // The coverage the records must show, as the class's tests read them.
    const need = [
        "api.delegatesPage:200", "api.delegatesPage:404", "api.delegatesPage:422", "api.delegatesPage:500",
        "api.walletsPage:200", "api.walletsPage:422", "api.walletsPage:500", "api.delegateResource:500",
    ];
    for (const kind of need) if (!seen.statuses.has(kind)) throw new Error(`V34 api-state: no record of ${kind}`);
    return out;
};
