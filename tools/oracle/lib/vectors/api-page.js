"use strict";
// The records of kinds api.searchPage and js.rbTrace in V34: the pagination service's getPage
// through getTop's red-black tree, over lists whose values the tree does and does not compare (a
// comparison of mixed types, of plain objects or of a Semver that is not valid is the route's
// 500, and which pairs the tree compares decides whether a request gets one), and the tree's
// comparison sequence over scripts of inserts and removes and over getTop of a list of values.

function generator(seed) {
    let state = seed >>> 0;
    return (limit) => {
        state = (state + 0x6d2b79f5) >>> 0;
        let t = state;
        t = Math.imul(t ^ (t >>> 15), t | 1);
        t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
        return (((t ^ (t >>> 14)) >>> 0) % limit);
    };
}

const big = (text) => ({ $bigNumber: text });
const semver = (text) => ({ $semver: text });

// Items with `v` as the value under sorting (a missing value is an absent key), numbered in order.
const items = (values) => values.map((v, id) => (v === undefined ? { id } : { id, v }));

const NUMBERS = [5, 3, 8, 1, 4, 9, 2, 7, 6, 0, -3, 12];
const WITH_TIES = [4, 2, 4, 1, 2, 4, 3, 1, 3, 2];
const STRINGS = ["pear", "apple", "fig", "Apple", "banana", "", "apple pie", "Zed", "zed", "10", "9"];
// UTF-16 order: an astral character (a surrogate pair) before a private-use one, and unlike the
// code point order.
const UTF16 = ["\u{1F600}", "�", "", "a", "é", "퟿", "～", "\u{10000}", "\u{10FFFF}", "\uFFFF", "z"];
const BIGS = ["5", "-3", "100000000000000000000", "0", "99999999999999999999", "7", "-100000000000000000000", "5"].map(big);
const SEMVERS = ["1.2.3", "1.10.0", "1.2.10", "0.9.0", "2.0.0", "1.2.3-beta.1", "1.2.3-alpha", "v1.0.0", "1.2.3+build"].map(semver);

const asc = (property = "v") => [{ property, direction: "asc" }];
const desc = (property = "v") => [{ property, direction: "desc" }];

const SORTED = [
    ["numbers", NUMBERS], ["ties", WITH_TIES], ["strings", STRINGS], ["utf-16", UTF16], ["bigNumbers", BIGS], ["semvers", SEMVERS],
    ["booleans", [true, false, true, false, false, true]], ["numbers with null", [3, null, 1, null, 2, 5]],
    ["numbers with absent", [3, undefined, 1, undefined, 2, 5]], ["null and absent", [null, undefined, 2, null, undefined, 1]],
];

const PAGES = [[0, 1], [0, 2], [0, 3], [1, 1], [1, 2], [2, 3], [3, 2], [0, 100], [7, 3], [40, 3]];

// Lists that mix types or hold plain objects: where the tree compares the pair the request is a 500.
const MIXED = [
    ["number and string", [1, "a"]], ["string and number", ["a", 1]], ["number, number, string", [1, 2, "a"]],
    ["string first of three", ["a", 1, 2]], ["boolean and number", [true, 1]], ["bigNumber and number", [big("1"), 2]],
    ["semver and string", [semver("1.0.0"), "1.0.0"]], ["number and bigNumber", [1, big("1")]], ["two objects", [{ a: 1 }, { a: 2 }]],
    ["object and number", [{ a: 1 }, 2]], ["array and array", [[1], [2]]], ["one object", [{ a: 1 }]], ["object then absent", [{ a: 1 }, undefined]],
    ["absent then object", [undefined, { a: 1 }]], ["object then null", [{ a: 1 }, null]], ["null then object", [null, { a: 1 }]],
    ["mixed after nulls", [null, null, 1, "a"]], ["number, absent, string", [1, undefined, "a"]], ["absent, absent, object", [undefined, undefined, { a: 1 }]],
    ["invalid semver", [semver("1.0.0"), semver("not a version")]], ["invalid semver first", [semver("not a version"), semver("1.0.0")]],
    ["two invalid semvers", [semver("x"), semver("y")]], ["semver with a v", [semver("v1.0.0"), semver("v2.0.0")]],
    ["mixed far apart", [1, 2, 3, 4, 5, "z"]], ["mixed deep", [9, 8, 7, 6, 5, 4, 3, 2, 1, "z"]], ["strings then number", ["a", "b", "c", "d", 1]],
];

function pageRecord(rec, height, name, list, sorting, offset, limit) {
    return rec("api.searchPage", height, { items: list, sorting, pagination: { offset, limit } }).then((record) => ({ ...record, name: `searchPage: ${name}, offset ${offset} limit ${limit}` }));
}

function script(random, size, distinct) {
    const values = Array.from({ length: size }, () => random(distinct));
    const steps = [];
    const held = [];
    for (let i = 0; i < size; i++) {
        if (held.length > 0 && random(3) === 0) steps.push(["remove", held.splice(random(held.length), 1)[0]]);
        else if (random(9) === 0) steps.push(["remove", random(size)]);
        else {
            steps.push(["insert", i]);
            held.push(i);
        }
    }
    while (held.length > 0 && random(2) === 0) steps.push(["remove", held.splice(random(held.length), 1)[0]]);
    return { values, script: steps };
}

module.exports = async function apiPageVectors({ rec, shared }) {
    const out = [];
    const height = shared.height;
    for (const [name, values] of SORTED) {
        const list = items(values);
        for (const [offset, limit] of PAGES) {
            for (const [order, sorting] of [["asc", asc()], ["desc", desc()]]) {
                if (name === "numbers" || offset + limit <= 3 || limit === 100 || order === "asc") out.push(await pageRecord(rec, height, `${name} ${order}`, list, sorting, offset, limit));
            }
        }
    }
    for (const [name, values] of MIXED) {
        for (const [offset, limit] of [[0, 1], [0, 2], [0, 3], [1, 2], [0, 100], [2, 1]])
            for (const sorting of [asc(), desc()]) out.push(await pageRecord(rec, height, `${name} ${sorting[0].direction}`, items(values), sorting, offset, limit));
    }
    // Totals below a page's size, and offsets past the end.
    for (const size of [0, 1, 2, 3, 4])
        for (const [offset, limit] of [[0, 3], [0, 100], [1, 3], [2, 2], [5, 3], [0, 2]])
            out.push(await pageRecord(rec, height, `${size} numbers`, items(NUMBERS.slice(0, size)), asc(), offset, limit));
    // Paths: nested objects, array and string indices, the markers' values, segments the reference
    // drops, and items that are not objects.
    const nested = [
        { id: 0, a: { b: 3 }, list: [4, 1], s: "ba" }, { id: 1, a: { b: 1 }, list: [2, 9], s: "ab" }, { id: 2, a: { b: null }, list: [], s: "" },
        { id: 3, a: null, list: [7], s: "c" }, { id: 4, list: [1, 1, 1], s: "\u{1F600}" }, { id: 5, a: { b: 2 }, list: [3, 3], s: "\u{1F601}" },
        { id: 6, a: { b: { c: 1 } }, list: [5], s: "b" }, { id: 7, a: [8, 6], list: [0], s: "zz" },
    ];
    const plain = nested.filter((item) => typeof (item.a || {}).b !== "object" || item.a.b === null);
    for (const path of ["a.b", "a.0", "list.0", "list.1", "list.length", "s.0", "s.1", "s.length", "list.01", "list.-1", "id.x", "a.b.c", "__proto__", "a.__proto__.b", "prototype", "constructor.name", "x"])
        for (const sorting of [asc(path), desc(path)])
            out.push({ ...(await rec("api.searchPage", height, { items: plain, sorting, pagination: { offset: 0, limit: 3 } })), name: `searchPage: path ${path} ${sorting[0].direction}, limit 3` });
    for (const path of ["a.b", "s.0", "s.1"])
        out.push({ ...(await rec("api.searchPage", height, { items: nested, sorting: asc(path), pagination: { offset: 0, limit: 2 } })), name: `searchPage: path ${path} over nested objects, limit 2` });
    const markers = [big("5"), big("3"), semver("1.0.0"), big("9")].map((v, id) => ({ id, v }));
    for (const path of ["v.value", "v"])
        out.push({ ...(await rec("api.searchPage", height, { items: markers.slice(0, 2).concat(markers.slice(3)), sorting: asc(path), pagination: { offset: 0, limit: 2 } })), name: `searchPage: marker path ${path}` });
    out.push({ ...(await rec("api.searchPage", height, { items: [big("1"), big("2")].map((v, id) => ({ id, v })), sorting: asc("v.value"), pagination: { offset: 0, limit: 5 } })), name: "searchPage: marker value" });
    out.push({ ...(await rec("api.searchPage", height, { items: [semver("1.0.0"), semver("0.5.0")].map((v, id) => ({ id, v })), sorting: asc("v.value"), pagination: { offset: 0, limit: 5 } })), name: "searchPage: semver value" });
    for (const [name, list] of [["numbers", [3, 1, 2]], ["strings", ["b", "a"]], ["booleans", [true, false]], ["with null", [null, 1]], ["nulls", [null, null, 1]]])
        out.push({ ...(await rec("api.searchPage", height, { items: list, sorting: asc("x"), pagination: { offset: 0, limit: 5 } })), name: `searchPage: items that are ${name}, path x` });
    out.push({ ...(await rec("api.searchPage", height, { items: [{ a: 1 }, { a: 0 }], sorting: asc("__proto__"), pagination: { offset: 0, limit: 5 } })), name: "searchPage: objects, dropped segment" });
    // Several properties: the first that differs decides, and the index breaks the last tie.
    const rows = [[1, "b"], [1, "a"], [0, "z"], [1, "a"], [0, "a"], [null, "a"], [undefined, "b"], [1, undefined], [2, null]].map(([v, w], id) => {
        const row = { id };
        if (v !== undefined) row.v = v;
        if (w !== undefined) row.w = w;
        return row;
    });
    for (const [offset, limit] of [[0, 3], [1, 2], [0, 100], [4, 3]]) {
        out.push(await pageRecord(rec, height, "two properties", rows, [...asc("v"), ...desc("w")], offset, limit));
        out.push(await pageRecord(rec, height, "two properties, second first", rows, [...asc("w"), ...asc("v")], offset, limit));
    }
    // The second property is reached only where the first ties, so a mix there is a 500 only then.
    const late = (pairs) => pairs.map(([v, w], id) => ({ id, v, w }));
    for (const [name, pairs] of [
        ["mixed second, first differs", [[1, "a"], [2, 5], [3, { x: 1 }]]],
        ["mixed second, first ties", [[1, "a"], [1, 5], [2, 0]]],
        ["mixed second, tie far off", [[1, "a"], [2, 5], [3, 1], [4, 2], [4, "x"]]],
        ["objects second, first differs", [[1, { x: 1 }], [2, { x: 2 }]]],
        ["objects second, first ties", [[1, { x: 1 }], [1, { x: 2 }]]],
    ])
        for (const [offset, limit] of [[0, 1], [0, 2], [0, 3], [0, 100]])
            out.push(await pageRecord(rec, height, name, late(pairs), [...asc("v"), ...asc("w")], offset, limit));
    for (const [name, values] of [["mixed", [1, "a", { a: 1 }]], ["numbers", [3, 1, 2]]])
        for (const offset of [0, 2]) out.push(await pageRecord(rec, height, `${name} with a zero limit`, items(values), asc(), offset, 0));
    out.push(await pageRecord(rec, height, "no sorting", items(NUMBERS), [], 2, 3));
    out.push(await pageRecord(rec, height, "no sorting", items(NUMBERS), [], 0, 100));
    out.push(await pageRecord(rec, height, "no sorting", items(NUMBERS), [], 40, 3));
    out.push(await pageRecord(rec, height, "zero limit", items(NUMBERS), asc(), 0, 0));
    out.push(await pageRecord(rec, height, "zero limit", items(NUMBERS), asc(), 3, 0));
    // The tree's comparison sequence.
    const random = generator(34);
    const scripts = [
        { values: [5, 3, 8, 1, 4], script: [["insert", 0], ["insert", 1], ["insert", 2], ["insert", 3], ["insert", 4], ["remove", 0], ["remove", 3]] },
        { values: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10], script: Array.from({ length: 10 }, (_, i) => ["insert", i]) },
        { values: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10], script: Array.from({ length: 10 }, (_, i) => ["insert", 9 - i]) },
        { values: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10], script: [...Array.from({ length: 10 }, (_, i) => ["insert", i]), ...Array.from({ length: 10 }, (_, i) => ["remove", i])] },
        { values: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10], script: [...Array.from({ length: 10 }, (_, i) => ["insert", i]), ...Array.from({ length: 10 }, (_, i) => ["remove", 9 - i])] },
        { values: [2, 2, 2, 2], script: [["insert", 0], ["insert", 1], ["insert", 2], ["insert", 3], ["remove", 2], ["remove", 0]] },
        { values: [1, 2, 3], script: [["remove", 0], ["insert", 1], ["remove", 2], ["remove", 1]] },
        { values: [], script: [] },
        { values: [7], script: [["insert", 0], ["remove", 0], ["remove", 0]] },
    ];
    for (let i = 0; i < 24; i++) scripts.push(script(random, 6 + random(30), i % 3 === 0 ? 4 : 1000));
    for (const [index, input] of scripts.entries()) out.push({ ...(await rec("js.rbTrace", height, input)), name: `rbTrace: script ${index} of ${input.script.length} steps` });
    const lists = [[5, 3, 8, 1, 4], [1, 2, 3, 4, 5, 6], [6, 5, 4, 3, 2, 1], [2, 2, 2, 2, 2], [1, 2, 1, 2, 1, 2, 1], [], [9]];
    for (let i = 0; i < 6; i++) lists.push(Array.from({ length: 8 + random(20) }, () => random(i % 2 === 0 ? 5 : 1000)));
    for (const [index, values] of lists.entries())
        for (const count of [0, 1, 2, 3, 5, 100])
            if (count !== 100 || index % 2 === 0) out.push({ ...(await rec("js.rbTrace", height, { values, count })), name: `rbTrace: top ${count} of list ${index}` });
    return out;
};
