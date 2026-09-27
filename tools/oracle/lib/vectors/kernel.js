"use strict";
// The kernel's and the logger's vector classes:
//   V09-rounds  calculateRound and isNewRound over heights 1 to 10,000 and around 2^32, under the
//               devnet's 53 delegates and under other delegate counts; calculateForgingInfo over
//               slot edges, half slots and gaps; isBlockChained cases;
//   V16-kernel  stringify, .env text, epoch timestamps, IP lists and node paths;
//   V20-logger  log lines for every level, coloured and plain, with and without emoji, over the
//               whole emoji table of the reference's logger.

const path = require("path");

const { milestonesText } = require("./milestones");

const TWO_32 = 4294967296;
const MAX_SAFE = Number.MAX_SAFE_INTEGER;

/** A milestone file with this delegate count, block time or epoch (JSON text values). */
function variant({ activeDelegates, blockTime, epoch } = {}) {
    const overrides = {};
    if (activeDelegates !== undefined) overrides.activeDelegates = String(activeDelegates);
    if (blockTime !== undefined) overrides.blockTime = String(blockTime);
    if (epoch !== undefined) overrides.epoch = JSON.stringify(epoch);
    return milestonesText(overrides);
}

async function rounds({ rec, must, shared }) {
    const out = [];
    const H = shared.height;
    const push = async (op, input, name) => out.push({ ...must(await rec(op, H, input)), name });

    // Under the devnet: 53 delegates, 8-second blocks.
    const edges = [1, 2, 3, 52, 53, 54, 55, 105, 106, 107, 108, 159, 160, 161, 1000, 5353, 5354, 2 ** 31 - 1, 2 ** 31, 2 ** 31 + 1];
    for (const h of [...edges, TWO_32 - 54, TWO_32 - 53, TWO_32 - 52, TWO_32 - 2, TWO_32 - 1]) {
        await push("round.calculate", { height: h }, `height ${h}`);
        await push("round.isNew", { height: h }, `height ${h}`);
    }
    for (const [from, to] of [
        [1, 10000],
        [TWO_32 - 10000, TWO_32 - 1],
    ]) {
        await push("round.calculate", { from, to }, `heights ${from} to ${to}`);
        await push("round.isNew", { from, to }, `heights ${from} to ${to}`);
    }

    // Other delegate counts, from a milestone file of their own.
    for (const n of [1, 2, 3, 7, 51, 52, 100, 2 ** 31, TWO_32 - 1, TWO_32, MAX_SAFE]) {
        const milestones = variant({ activeDelegates: n });
        for (const h of [1, 2, 3, 7, 8, 51, 52, 53, 100, 101, TWO_32 - 1]) {
            await push("round.calculate", { height: h, milestones }, `${n} delegates, height ${h}`);
        }
        await push("round.calculate", { from: 1, to: 2000, milestones }, `${n} delegates, heights 1 to 2000`);
        await push("round.isNew", { from: 1, to: 2000, milestones }, `${n} delegates, heights 1 to 2000`);
        await push("round.isNew", { from: TWO_32 - 2000, to: TWO_32 - 1, milestones }, `${n} delegates, heights near 2^32`);
    }

    // The forging slot: 8-second slots, forging allowed in the first 4 seconds.
    const times = [0, 1, 3, 4, 5, 7, 8, 11, 12, 15, 16, 423, 424, 427, 428, 431, 432, 435, 436, 3371, 3372, 1000000, 2 ** 31 - 1, TWO_32 - 8, TWO_32 - 5, TWO_32 - 4, TWO_32 - 1];
    for (const t of [...times, -1, -4, -5, -8, -9, -424, -425, -(2 ** 31)]) {
        for (const h of [1, 2, 54]) await push("forging.info", { timestamp: t, height: h }, `time ${t}, height ${h}`);
    }
    // A block far from its slot (the index follows the time, not the height).
    await push("forging.info", { timestamp: 800, height: 3 }, "a gap of 97 slots");
    await push("forging.info", { timestamp: 8, height: 10000 }, "height after the time");
    for (const [blockTime, activeDelegates] of [
        [1, 1],
        [2, 3],
        [3, 7],
        [9, 53],
        [10, 51],
        [60, 2],
        [1, MAX_SAFE],
        [MAX_SAFE, 53],
    ]) {
        const milestones = variant({ blockTime, activeDelegates });
        for (const t of [0, 1, 2, 3, 4, 5, 8, 9, 17, 18, 59, 60, 61, 119, 120, 1000, TWO_32 - 1, -1, -9, -61]) {
            await push("forging.info", { timestamp: t, height: 2, milestones }, `block time ${blockTime}, ${activeDelegates} delegates, time ${t}`);
        }
    }

    // Chaining: the previous block's id, the height plus one, a later slot, in this order.
    const A = "a1".repeat(32);
    const B = "b2".repeat(32);
    const C = "c3".repeat(32);
    const chained = async (previous, next, name, milestones) => push("block.isChained", { previous, next, ...(milestones ? { milestones } : {}) }, name);
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: A, height: 6, timestamp: 24 }, "chained");
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: A, height: 6, timestamp: 1000 }, "chained after a gap");
    await chained({ id: A, height: 1, timestamp: 0 }, { id: B, previousBlock: A, height: 2, timestamp: 8 }, "chained on the genesis block");
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: C, height: 6, timestamp: 24 }, "other previous block");
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: A, height: 5, timestamp: 24 }, "same height");
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: A, height: 7, timestamp: 24 }, "height plus two");
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: A, height: 4, timestamp: 24 }, "lower height");
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: A, height: 6, timestamp: 23 }, "same slot");
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: A, height: 6, timestamp: 16 }, "same time");
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: A, height: 6, timestamp: 8 }, "earlier slot");
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: C, height: 7, timestamp: 8 }, "every check fails");
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: A, height: 7, timestamp: 8 }, "height and slot fail");
    await chained({ id: A, height: 5, timestamp: 16 }, { id: B, previousBlock: C, height: 6, timestamp: 8 }, "id and slot fail");
    await chained({ id: A, height: TWO_32 - 1, timestamp: 16 }, { id: B, previousBlock: A, height: TWO_32 - 1, timestamp: 24 }, "previous at 2^32 - 1");
    await chained({ id: A, height: 5, timestamp: TWO_32 - 9 }, { id: B, previousBlock: A, height: 6, timestamp: TWO_32 - 1 }, "times near 2^32");
    const nine = variant({ blockTime: 9 });
    await chained({ id: A, height: 5, timestamp: 17 }, { id: B, previousBlock: A, height: 6, timestamp: 18 }, "9-second slots, next slot", nine);
    await chained({ id: A, height: 5, timestamp: 9 }, { id: B, previousBlock: A, height: 6, timestamp: 17 }, "9-second slots, same slot", nine);
    return out;
}

/** An object with keys in the given order, own "__proto__" included, as JSON.parse builds it. */
function objectFrom(text) {
    return JSON.parse(text);
}

async function kernel({ rec, must, shared }) {
    const out = [];
    const H = shared.height;
    const push = async (op, input, name) => out.push({ ...(await rec(op, H, input)), name });
    const pushOk = async (op, input, name) => out.push({ ...must(await rec(op, H, input)), name });

    // The key-sorted JSON of the relay status.
    const values = [
        ["an empty object", {}],
        ["an empty array", []],
        ["null", null],
        ["a string", "text"],
        ["a number", 42],
        ["keys in reverse order", { c: 3, b: 2, a: 1 }],
        ["nested objects", { b: { d: 1, c: [2, { f: 3, e: 4 }] }, a: true }],
        ["arrays of arrays", { a: [[3, 2], [], [{ y: null, x: false }]] }],
        ["the same key at two depths", { a: { a: { a: 1 } } }],
        ["numeric keys", objectFrom('{"10":1,"2":2,"1":3,"01":4,"a":5,"-1":6,"4294967295":7,"4294967294":8}')],
        ["an empty key", objectFrom('{"":1,"a":{"":2}}')],
        ["prototype names are left out", objectFrom('{"constructor":1,"toString":2,"valueOf":3,"hasOwnProperty":4,"__proto__":5,"isPrototypeOf":6,"propertyIsEnumerable":7,"toLocaleString":8,"__defineGetter__":9,"__defineSetter__":10,"__lookupGetter__":11,"__lookupSetter__":12,"kept":13}')],
        ["a prototype name inside an array", objectFrom('[{"constructor":1,"b":2}]')],
        ["keys sorted by UTF-16 code units", objectFrom('{"\\uffff":1,"\\ud83d\\ude00":2,"\\u00e9":3,"z":4,"Z":5,"_":6,"\\u0000":7}')],
        ["numbers", { a: 1e21, b: 1e-7, c: 0.1, d: -0, e: 5e-324, f: 1.7976931348623157e308, g: 123456789012345680000, h: -1.5, i: 100 }],
        ["strings with escapes", { a: 'q"b\\n\n\r\t\b\f\u0001\u001f\u007f', b: "  ", c: "😀 é" }],
        [
            "a relay status",
            {
                config: {
                    version: "4.3.1",
                    network: { version: 90, name: "devnet", nethash: "00".repeat(32), explorer: "", token: { name: "dROOT", symbol: "dRT" } },
                    plugins: { "@solar-network/api": { enabled: true, port: 6003, estimateTotalCount: true } },
                },
                state: { height: 54, forgingAllowed: true, currentSlot: 53, header: { id: "ab".repeat(32), height: 54 } },
                publicKeys: ["02" + "11".repeat(32)],
                signatures: [],
            },
        ],
    ];
    for (const [name, value] of values) await pushOk("kernel.stringify", { value }, name);

    // .env text.
    const texts = [
        ["pairs", "A=1\nB=two\nC=true\nD=false"],
        ["colon separator", "A:1\nB: two"],
        ["the first separator splits", "A=b=c\nB:c=d\nC=d:e"],
        ["comments and blank lines", "# a comment\n\nA=1\n  # indented comment\n#B=2"],
        ["a hash inside the key", "A#B=1\nC=d#e"],
        ["quotes", 'A="x"\nB=" x "\nC=""\nD="\nE=\'x\'\nF= "x"\nG="a"b"\nH="""'],
        ["digits", "A=0\nB=00\nC=007\nD=42\nE= 42 \nF=1e5\nG=-1\nH=1.5\nI=99999999999999999999999\nJ=9007199254740993"],
        ["digits beyond a double", `A=${"9".repeat(400)}`],
        ["booleans", "A=true\nB=false\nC=True\nD=FALSE\nE= true"],
        ["empty keys and values", "A=\n=x\n  =y\nB=  \nC"],
        ["CRLF lines", "A=1\r\nB=two\r\nC\r=3"],
        ["line separators", "A=a b\nB=c d\nC =e"],
        ["whitespace JavaScript trims", "﻿A=1\n B =　two　\n\u000bC\u000c=3"],
        ["whitespace JavaScript keeps", "A\u0085=1\nB=\u0085two\u0085"],
        ["a repeated key keeps its first place", "A=1\nB=2\nA=3"],
        ["array-index keys first, in numeric order", "b=1\n10=2\n2=3\n01=4\n4294967294=5\n4294967295=6\n0=7\na=8"],
        ["prototype names", "__proto__=1\nconstructor=2\ntoString=three\nhasOwnProperty=true"],
        ["export and spaces in keys", "export A=1\nB C=2\n  D  =  3  "],
        ["tabs", "\tA\t=\t1\t\nB=\t\"x\"\t"],
        ["unicode", "ÉTAT=été\n😀=😁"],
        ["no final newline and a final newline", "A=1\n"],
        ["an empty text", ""],
    ];
    for (const [name, text] of texts) await pushOk("kernel.dotenv", { text }, name);

    // Epoch timestamps under the devnet epoch, 2026-01-01T00:00:00.000Z.
    const EPOCH_MS = Date.parse("2026-01-01T00:00:00.000Z");
    const lastValid = Math.floor((8.64e15 - EPOCH_MS) / 1000);
    const firstValid = Math.ceil((-8.64e15 - EPOCH_MS) / 1000);
    // Date.UTC reads years 0 to 99 as 1900 to 1999, so the first second of a year is set with
    // setUTCFullYear.
    const toYear = (year) => {
        const date = new Date(0);
        date.setUTCFullYear(year, 0, 1);
        return Math.floor((date.getTime() - EPOCH_MS) / 1000);
    };
    const stamps = [0, 1, 8, -1, 59, 60, 86399, 86400, 1e9, 2 ** 31 - 1, TWO_32 - 1, -EPOCH_MS / 1000, -EPOCH_MS / 1000 - 1];
    stamps.push(toYear(999), toYear(1000) - 1, toYear(10000) - 1, toYear(10000), toYear(0), toYear(0) - 1, toYear(-1), toYear(1900), toYear(1970) + 59 * 86400 + 86399);
    stamps.push(lastValid, lastValid + 1, firstValid, firstValid - 1, MAX_SAFE, -MAX_SAFE);
    for (const timestamp of stamps) await push("kernel.formatTimestamp", { timestamp }, `time ${timestamp}`);
    for (const epoch of ["2022-03-28T18:00:00.000Z", "2026-01-01T02:00:00+02:00", "2025-12-31T23:00:00-01:00", "2026-01-01T00:00:00.123456Z", "2026-01-01T00:00Z", "2024-02-29T12:34:56.789Z"]) {
        const milestones = variant({ epoch });
        for (const timestamp of [0, 1, -1, 86400 * 365]) await push("kernel.formatTimestamp", { timestamp, milestones }, `epoch ${epoch}, time ${timestamp}`);
    }

    // IP addresses and lists.
    const addresses = [
        "127.0.0.1",
        "10.0.0.1",
        "10.0.0.2",
        "10.0.0.12",
        "192.168.1.5",
        "192.168.1.50",
        "::1",
        "::ffff:127.0.0.1",
        "fe80::1",
        "FE80::1",
        "2001:db8::8a2e:370:7334",
        "[::1]",
        "[2001:db8::1]",
        "fe80::1%eth0",
        ".1.2.3",
    ];
    const lists = [
        [],
        ["*"],
        ["**"],
        ["127.0.0.1"],
        ["127.0.0.*"],
        ["10.0.0.?"],
        ["10.0.0.??"],
        ["?0.0.0.1"],
        ["?.?.?.?"],
        ["*.*.*.*"],
        ["192.168.1.[0-9]"],
        ["192.168.1.[!0-4]*"],
        ["192.168.1.[^5]0"],
        ["1[0-9].0.0.*"],
        ["!10.0.0.1"],
        ["!!10.0.0.1"],
        ["!"],
        ["::1"],
        ["::ffff:127.0.0.1"],
        ["::ffff:*"],
        ["*:*"],
        ["fe80::*"],
        ["[::1]"],
        ["[:]:1"],
        ["1.2.3.[9-0]"],
        ["192.168.1.[!4-]"],
        ["10..0.1*"],
        ["10.0.0.1", "10.0.0.2"],
        ["10.0.0.3", "192.168.*"],
        ["*.1.2.3"],
        ["?1.2.3"],
    ];
    for (const list of lists) {
        for (const ip of addresses) await pushOk("kernel.ip", { list, ip }, `${JSON.stringify(list)} ${ip}`);
    }
    const oddAddresses = [
        "",
        "abc",
        "0",
        "127.1",
        "127.0.1",
        "0x7f.1",
        "0X7F.0.0.1",
        "010.0.0.1",
        "08.0.0.1",
        "1.2.3.256",
        "1.2.3.04",
        "0x100.1.1.1",
        "4294967295",
        "4294967296",
        "0xffffffff",
        "1.16777215",
        "1.16777216",
        "1.2.65535",
        "1.2.65536",
        "::",
        ":::",
        "1::2::3",
        "1:2:3:4:5:6:7:8",
        "1:2:3:4:5:6:7:8:9",
        "1:2:3:4:5:6:7::",
        "::1:2:3:4:5:6:7",
        "1::",
        "1:",
        ":1",
        "fffff::1",
        "0000000000001::1",
        "::1.2.3.4",
        "::0x1.2.3.4",
        "1::1.2.3.4",
        "1:2:3:4:5:6:1.2.3.4",
        "::ffff:1.2.3.256",
        "::ffff:1.2.3.4%1",
        "fe80::1%",
        "fe80::1%eth0%1",
        "[127.0.0.1]",
        "[[::1]]",
        "]::1[",
        "::1]",
    ];
    for (const ip of oddAddresses) await pushOk("kernel.ip", { list: ["*", "::*"], ip }, `address ${JSON.stringify(ip)}`);

    // Bracket forms of the glob matcher: squares, [!] and [^], a [ that no ] closes (the last
    // character left to the regular expression), [] and stray ] and ^, and a class that breaks
    // the regular expression.
    const bracketAddresses = ["[12", "[1.2", "[x.y", "[xy", "1]2", "a^b", "!", "[", "]", "[]", "a!b", "[[e%4", "x"];
    const bracketEntries = ["[]]", "[[]", "[!]", "[^]", "[!12", "[12", "[1.2", "[x.", "[x^", "[x[", "[!x", "[^xy?", "[x?*", "[*", "[]", "[]x", "1]2", "a^b", "a!b", "![[[e]%4", "!!c[%c", "[[e]%4"];
    for (const entry of bracketEntries) {
        for (const ip of bracketAddresses) await pushOk("kernel.ip", { list: [entry], ip }, `${JSON.stringify(entry)} ${JSON.stringify(ip)}`);
    }

    // Node directories on Linux.
    const paths = [
        ["defaults", { token: "solar", network: "devnet", home: "/srv/node" }],
        ["another token and network", { token: "root", network: "testnet", home: "/root" }],
        ["a home with a trailing slash", { token: "solar", network: "devnet", home: "/srv/node/" }],
        ["the root as home", { token: "solar", network: "devnet", home: "/" }],
        ["an empty network", { token: "solar", network: "", home: "/srv/node" }],
        ["a network with dots", { token: "solar", network: "../up", home: "/srv/node" }],
        ["every CORE_PATH variable", { token: "solar", network: "devnet", home: "/srv/node", env: { CORE_PATH_DATA: "/data", CORE_PATH_CONFIG: "/etc/solar/", CORE_PATH_CACHE: "/var/cache//solar", CORE_PATH_LOG: "/var/log/solar/../logs", CORE_PATH_TEMP: "/tmp/./solar" } }],
        ["relative CORE_PATH variables", { token: "solar", network: "devnet", home: "/srv/node", cwd: "/usr", env: { CORE_PATH_DATA: "data", CORE_PATH_LOG: "../log" } }],
        ["tildes", { token: "solar", network: "devnet", home: "/srv/node", cwd: "/usr", env: { CORE_PATH_DATA: "~/data", CORE_PATH_CONFIG: "~", CORE_PATH_CACHE: "~+/cache", CORE_PATH_LOG: "~user/log", CORE_PATH_TEMP: "~+" } }],
        ["empty CORE_PATH variables", { token: "solar", network: "devnet", home: "/srv/node", env: { CORE_PATH_DATA: "", CORE_PATH_TEMP: "" } }],
        ["XDG variables", { token: "solar", network: "devnet", home: "/srv/node", env: { XDG_DATA_HOME: "/xdg/data", XDG_CONFIG_HOME: "/xdg/config/", XDG_CACHE_HOME: "relative", XDG_STATE_HOME: "" } }],
        ["TMPDIR", { token: "solar", network: "devnet", home: "/srv/node", env: { TMPDIR: "/var/tmp/" } }],
        ["TMP and TEMP", { token: "solar", network: "devnet", home: "/srv/node", env: { TMP: "/tmp2", TEMP: "/tmp3" } }],
        ["TEMP", { token: "solar", network: "devnet", home: "/srv/node", env: { TMPDIR: "", TEMP: "/tmp3" } }],
        ["a root TMPDIR", { token: "solar", network: "devnet", home: "/srv/node", env: { TMPDIR: "/" } }],
        ["a relative TMPDIR", { token: "solar", network: "devnet", home: "/srv/node", cwd: "/usr", env: { TMPDIR: "tmp" } }],
    ];
    for (const [name, input] of paths) await pushOk("kernel.paths", input, name);
    return out;
}

async function logger({ rec, must, shared, solar }) {
    const out = [];
    const H = shared.height;
    const push = async (input, name) => out.push({ ...must(await rec("logger.format", H, input)), name });
    const LEVELS = ["emergency", "alert", "critical", "error", "warning", "notice", "info", "debug"];
    const T = 1767225608123; // 2026-01-01T00:00:08.123Z

    for (const level of LEVELS) {
        for (const colorize of [true, false]) {
            for (const emojify of [true, false]) {
                await push({ level, time: T, message: `Starting Round 2 :dove_of_peace: at height 54 :warning:`, colorize, emojify }, `${level}, colorize ${colorize}, emojify ${emojify}`);
            }
        }
    }

    const messages = [
        ["plain text", "Loaded 53 delegates"],
        ["an empty message", ""],
        ["only an emoji code", ":zap:"],
        ["an unknown code", "a :not_an_emoji: b"],
        ["unfinished codes", ":warning a: b :"],
        ["adjacent codes", "::warning::zap:::"],
        ["codes with digits and signs", ":+1: :-1: :100: :1234:"],
        ["USERLVL in the message", "USERLVL and USERLVL"],
        ["line breaks", "first\nsecond\r\nthird\n"],
        ["colour codes in the message", "a \u001b[39m b \u001b[36m c \u001b[31mred\u001b[39m"],
        ["unicode", "état 😀   end"],
        ["JSON-like text", '{"level":"info","msg":"x"}'],
        ["percent signs", "100% %s %d %j"],
    ];
    for (const [name, message] of messages) {
        for (const level of ["info", "warning", "error", "debug", "emergency"]) {
            for (const [colorize, emojify] of [
                [true, true],
                [false, false],
                [true, false],
                [false, true],
            ]) {
                await push({ level, time: T, message, colorize, emojify }, `${name}, ${level}, colorize ${colorize}, emojify ${emojify}`);
            }
        }
    }

    const times = [0, 1, T, 1767225608999, 253402300799999, 253402300800000, -1, -62167219200000, -62198755200001, 8.64e15, -8.64e15, 8.64e15 + 1, -8.64e15 - 1];
    for (const time of times) {
        for (const colorize of [true, false]) await push({ level: "notice", time, message: "time", colorize, emojify: true }, `time ${time}, colorize ${colorize}`);
    }

    // Every name of the emoji table, 64 to a message.
    const loggerDir = path.join(solar.solarDir, "packages", "logger");
    const table = require(require.resolve("node-emoji", { paths: [loggerDir] })).emoji;
    const names = Object.keys(table);
    for (let i = 0; i < names.length; i += 64) {
        const message = `emoji ${i}: ${names
            .slice(i, i + 64)
            .map((name) => `:${name}:`)
            .join(" ")} end`;
        for (const emojify of [true, false]) await push({ level: "info", time: T, message, colorize: false, emojify }, `emoji table from ${i}, emojify ${emojify}`);
    }
    return out;
}

module.exports = { rounds, kernel, logger };
