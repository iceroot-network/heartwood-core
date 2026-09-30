"use strict";
// The records of kinds api.spec, api.transactionSchemas, api.feeStatistics and api.fuzzQuery in
// V34.
//
// api.spec: the document under both address version bytes the networks use (90 and 30), with the
// pool's request limit and the blockchain's lookback varied: the lookbacks are the ones whose days
// the reference writes with a tie (0.125, 0.375, 0.625 and 0.875 of a day, where toFixed takes the
// larger candidate), whole days, and the ones that round away.
// api.transactionSchemas: the schemas under both version bytes.
// api.feeStatistics: fee rows of the kept types as the repository returns them (one per type, as
// text), listed with and without days, with the types in the order the rows have, and empty.
// api.fuzzQuery: a choice byte and a raw query, through the reference's query plugins, the route's
// validation and the pagination plugin, on the routes the chain routes added, on a listing, and on
// each route of the state and history listings (choice bytes 11 to 26).
// Inputs are fixed, so every run writes the same records.

const LOOKBACKS = [604800, 129600, 10800, 32400, 54000, 75600, 1000, 100, 0, 2592000, 86400, 1];

// The kept types, as [typeGroup, type], in the registry's order (listed reversed below).
const KEPT = [[1, 1], [1, 2], [1, 6], [1, 7], [2, 0], [2, 2]];

const row = (typeGroup, type, avg, burned, max, min, sum) => ({ typeGroup, type, avg, burned, max, min, sum });

// Fee rows: [name, days, rows].
const FEES = [
    ["no rows", undefined, []],
    ["no rows, one day", 1, []],
    ["one transfer type", undefined, [row(1, 6, "10000000", "0", "10000000", "10000000", "200000000")]],
    ["one type, thirty days", 30, [row(1, 6, "5000000", "0", "9000000", "1000000", "15000000")]],
    ["every kept type, in the registry's order", undefined, [
        row(1, 1, "500000000", "0", "500000000", "500000000", "500000000"),
        row(1, 2, "2500000000", "0", "2500000000", "2500000000", "2500000000"),
        row(1, 6, "10000000", "0", "12000000", "9000000", "200000000"),
        row(1, 7, "2500000000", "0", "2500000000", "2500000000", "2500000000"),
        row(2, 0, "0", "25000000", "0", "0", "0"),
        row(2, 2, "100000000", "0", "100000000", "100000000", "100000000"),
    ]],
    ["every kept type, in the export order of the handlers module", undefined, [
        row(1, 2, "2500000000", "0", "2500000000", "2500000000", "2500000000"),
        row(1, 7, "2500000000", "0", "2500000000", "2500000000", "2500000000"),
        row(1, 6, "10000000", "0", "12000000", "9000000", "200000000"),
        row(1, 1, "500000000", "0", "500000000", "500000000", "500000000"),
        row(2, 0, "0", "25000000", "0", "0", "0"),
        row(2, 2, "100000000", "0", "100000000", "100000000", "100000000"),
    ]],
    ["every kept type, seven days, in reverse", 7, KEPT.slice().reverse().map(([group, type], at) => row(group, type, String(at * 1000), String(at), String(at * 1000 + 5), String(at * 1000 - 5 < 0 ? 0 : at * 1000 - 5), String(at * 4000)))],
    ["values beyond 2^53", 1, [row(1, 6, "9007199254740993", "9223372036854775807", "9223372036854775807", "1", "18446744073709551615")]],
    ["a type in a group of its own", 30, [row(2, 0, "1", "123456789012345678", "1", "1", "1")]],
    ["the same type twice", undefined, [row(1, 6, "1", "0", "1", "1", "1"), row(1, 6, "2", "0", "2", "2", "2")]],
    ["the votes only", 15, [row(2, 2, "300", "0", "500", "100", "900")]],
];

// The choice bytes of the routes the chain routes added: 7 (blockchain), 8 (search), 9 (fees) and
// 10 (schemas), and 0 (a listing).
const QUERIES = [
    [9, ""],
    [9, "days=1"],
    [9, "days=30"],
    [9, "days=7"],
    [9, "days=0"],
    [9, "days=31"],
    [9, "days=1.5"],
    [9, "days=abc"],
    [9, "days="],
    [9, "days=1e1"],
    [9, "days=%2012"],
    [9, "days=+7"],
    [9, "days=07"],
    [9, "days=-3"],
    [9, "days=9007199254740992"],
    [9, "days=1e400"],
    [9, "days=1&days=2"],
    [9, "days=1,2"],
    [9, "days[]=1"],
    [9, "days.a=1"],
    [9, "page=2"],
    [9, "days=5&limit=3"],
    [7, ""],
    [7, "page=2&limit=5"],
    [7, "anything=goes"],
    [8, ""],
    [8, "limit=1"],
    [10, ""],
    [10, "x=1"],
    [0, "limit=5&page=2"],
    [0, "days=1"],
    [0, "limit=101"],
];

// The choice bytes of the state and history listings (11 to 26, in the target's order), each with
// no query, the queries its schema accepts and some it refuses.
const LISTING_QUERIES = [
    [11, ""],
    [11, "orderBy=rank:asc&limit=5"],
    [11, "username=genesis_1&isResigned=false"],
    [11, "producedBlocks.from=1&orderBy=producedBlocks:desc"],
    [11, "limit=101"],
    [11, "nosuch=1"],
    [12, ""],
    [12, "anything=goes"],
    [13, ""],
    [13, "balance.from=1&orderBy=balance:desc"],
    [13, "attributes.delegate.username=genesis_1"],
    [13, "attributes.*.username=genesis_1"],
    [14, ""],
    [14, "orderBy=balance:desc,nonce:asc&page=2&limit=3"],
    [14, "publicKey=02%25"],
    [14, "attributes.secondPublicKey=03aa&votingFor.genesis_1.votes.from=1"],
    [14, "nosuch=1"],
    [15, ""],
    [15, "limit=2&balance.to=100"],
    [15, "orderBy=attributes.nosuch:asc"],
    [16, ""],
    [16, "orderBy=height:desc&limit=3"],
    [16, "username=genesis_1&height.from=2&height.to=5"],
    [16, "orderBy=nosuch:asc"],
    [16, "transform=false&payloadHash=zz"],
    [17, ""],
    [17, "username=genesis_2&orderBy=timestamp:asc"],
    [17, "page=2&limit=1&height.from=3"],
    [18, ""],
    [18, "transform=false"],
    [18, "transform=maybe"],
    [19, ""],
    [19, "orderBy=height:asc&limit=2"],
    [19, "transform=false&totalFee.from=0"],
    [20, ""],
    [20, "timestamp.from=1&orderBy=height:desc"],
    [20, "totalAmount=1"],
    [21, ""],
    [21, "a=1&page=2"],
    [22, ""],
    [22, "orderBy=timestamp:desc&limit=3"],
    [22, "senderPublicKey=02aa&transform=false"],
    [23, ""],
    [23, "transform=false"],
    [24, ""],
    [24, "orderBy=amount:desc&limit=2"],
    [24, "type=0&typeGroup=2"],
    [25, ""],
    [25, "typeGroup=1&nonce.from=1"],
    [26, ""],
    [26, "page=2&limit=1"],
    [26, "orderBy=nosuch:asc"],
];

const hex = (choice, query) => Buffer.concat([Buffer.from([choice]), Buffer.from(query, "utf8")]).toString("hex");

module.exports = async function apiSpecVectors({ rec, must, shared }) {
    const out = [];
    const H = shared.height;
    const add = async (op, input, name) => out.push({ ...must(await rec(op, H, input)), name });

    const base = (pubKeyHash, maxTransactionsPerRequest, missedBlocksLookback, basePath, version) => ({
        network: { pubKeyHash },
        pool: { maxTransactionsPerRequest },
        blockchain: { missedBlocksLookback },
        basePath,
        version,
    });
    await add("api.spec", base(90, 40, 604800, "/api", "4.3.1"), "spec: network 90, defaults");
    await add("api.spec", base(30, 150, 604800, "/api", "4.3.1"), "spec: network 30, a larger request limit");
    for (const seconds of LOOKBACKS) {
        await add("api.spec", base(90, 40, seconds, "/api", "4.3.1"), `spec: lookback ${seconds}`);
    }
    await add("api.spec", base(30, 1, 10800, "/v2", "0.1.0-test"), "spec: network 30, another base path and version");
    await add("api.spec", base(23, 40, 604800, "", "1"), "spec: network 23, no base path");
    await add("api.spec", base(63, 2 ** 31, 129600, "/api", "1.0.0"), "spec: network 63, a very large request limit");

    await add("api.transactionSchemas", { network: { pubKeyHash: 90 } }, "transactionSchemas: network 90");
    await add("api.transactionSchemas", { network: { pubKeyHash: 30 } }, "transactionSchemas: network 30");

    for (const [name, days, rows] of FEES) {
        const input = days === undefined ? { rows } : { rows, days };
        await add("api.feeStatistics", input, `feeStatistics: ${name}`);
    }

    for (const [choice, query] of [...QUERIES, ...LISTING_QUERIES]) {
        await add("api.fuzzQuery", { hex: hex(choice, query) }, `fuzzQuery: route ${choice}, ${JSON.stringify(query)}`);
    }
    // Nested keys: to the depth the records write out whole, and past it, where they give the depth.
    for (const levels of [50, 98, 99, 100, 101, 127, 128, 129, 130, 131, 132, 133, 140]) {
        await add("api.fuzzQuery", { hex: hex(0, `a${".k".repeat(levels - 1)}=1`) }, `fuzzQuery: a key nested ${levels} levels`);
    }
    // A key that takes any object, nested to the grammar's bound and past it: asset on a
    // transaction list (0) and on a wallet's votes (26), attributes on the wallets (14).
    for (const [choice, key] of [[0, "asset"], [26, "asset"], [14, "attributes"]]) {
        for (const levels of [128, 129, 130, 200]) {
            await add("api.fuzzQuery", { hex: hex(choice, `${key}${".k".repeat(levels - 1)}=1`) }, `fuzzQuery: route ${choice}, ${key} nested ${levels} levels`);
        }
    }
    await add("api.fuzzQuery", { hex: "" }, "fuzzQuery: no input");
    return out;
};
