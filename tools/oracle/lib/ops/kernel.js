"use strict";
// Oracle operations of the reference's kernel and logger. Loaded by lib/ops.js.
//
//   round.calculate         Utils.roundCalculator.calculateRound(height); with {from, to}, the
//                           SHA-256 of one line "h round roundHeight nextRound maxDelegates\n" per
//                           height of the range
//   round.isNew             Utils.roundCalculator.isNewRound(height); with {from, to}, the heights
//                           of the range that start a round
//   forging.info            Utils.forgingInfoCalculator.calculateForgingInfo(timestamp, height)
//   block.isChained         Utils.isBlockChained, and which check refuses the pair, from
//                           Utils.getBlockNotChainedErrorMessage
//   kernel.stringify        Utils.stringify, the key-sorted JSON that relays sign
//   kernel.dotenv           Utils.dotenv.parse, and Utils.dotenv.stringify of its result
//   kernel.formatTimestamp  Utils.formatTimestamp under the milestone epoch
//   kernel.ip               Utils.isWhitelisted and Utils.isBlacklisted of one list, and the
//                           Utils.IpAddress helpers of the address
//   kernel.paths            the directories the kernel's RegisterBasePaths bootstrapper derives
//   logger.format           the line the logger writes for one message: PinoLogger.log through
//                           pino and the pretty transport of the console (colorize) or the file
//
// Every operation that reads milestones takes an optional `milestones` override (JSON text), as
// fee.minimum does. The kernel's utilities are loaded file by file from its build, so the kernel's
// application container is never started; the logger's driver loads the kernel's index.

const path = require("path");

// The variables kernel.paths reads, which it sets for the call only.
const PATH_VARIABLES = [
    "HOME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "CORE_PATH_DATA",
    "CORE_PATH_CONFIG",
    "CORE_PATH_CACHE",
    "CORE_PATH_LOG",
    "CORE_PATH_TEMP",
];

// The custom levels of the reference's logger, as its make() configures pino.
const LEVELS = { emergency: 0, alert: 1, critical: 2, error: 3, warning: 4, notice: 5, info: 6, debug: 7 };

// The widest range one call may cover.
const MAX_RANGE = 100000;

module.exports = function kernelOps(context) {
    const { solar, atHeight, withConfig, safeInteger, sha256Hex } = context;
    const kernelDir = path.join(solar.solarDir, "packages", "kernel");
    const loggerDir = path.join(solar.solarDir, "packages", "logger");
    const util = (file) => require(path.join(kernelDir, "dist", "utils", file));

    // A getTimeStampForBlock lookup. With one span of block times and delegate counts (the loader
    // refuses a change after height 1), the reference never calls it.
    const noLookup = (height) => {
        throw new Error(`the block time lookup was called for height ${height}: more than one span`);
    };

    function range(from, to) {
        safeInteger(from, "from");
        safeInteger(to, "to");
        if (from < 1 || to < from || to - from >= MAX_RANGE) throw new RangeError(`from and to must be 1 <= from <= to < from + ${MAX_RANGE}`);
        return { from, to };
    }

    function blockData(value, name) {
        if (value === null || typeof value !== "object" || Array.isArray(value)) throw new TypeError(`${name} must be an object`);
        return value;
    }

    let logger;
    function loggerParts() {
        if (!logger) {
            const pino = require(require.resolve("pino", { paths: [loggerDir] }));
            const chalk = require(require.resolve("chalk", { paths: [loggerDir] }));
            const { PinoLogger } = require(path.join(loggerDir, "dist", "driver.js"));
            logger = { pino, chalk, PinoLogger };
        }
        return logger;
    }

    return {
        "round.calculate": ({ height, from, to, milestones }, ruleHeight) =>
            withConfig(milestones, () =>
                atHeight(ruleHeight, () => {
                    const { calculateRound } = util("round-calculator.js");
                    if (height !== undefined) {
                        const { round, roundHeight, nextRound, maxDelegates } = calculateRound(safeInteger(height, "height"));
                        return { round, roundHeight, nextRound, maxDelegates };
                    }
                    const r = range(from, to);
                    const lines = [];
                    for (let h = r.from; h <= r.to; h++) {
                        const info = calculateRound(h);
                        lines.push(`${h} ${info.round} ${info.roundHeight} ${info.nextRound} ${info.maxDelegates}\n`);
                    }
                    return { count: lines.length, sha256: sha256Hex(Buffer.from(lines.join(""), "utf8")) };
                }),
            ),

        "round.isNew": ({ height, from, to, milestones }, ruleHeight) =>
            withConfig(milestones, () =>
                atHeight(ruleHeight, () => {
                    const { isNewRound } = util("round-calculator.js");
                    if (height !== undefined) return { isNewRound: isNewRound(safeInteger(height, "height")) };
                    const r = range(from, to);
                    const newRounds = [];
                    for (let h = r.from; h <= r.to; h++) if (isNewRound(h)) newRounds.push(h);
                    return { count: r.to - r.from + 1, newRounds };
                }),
            ),

        "forging.info": ({ timestamp, height, milestones }, ruleHeight) =>
            withConfig(milestones, () =>
                atHeight(ruleHeight, () => {
                    const { calculateForgingInfo } = util("calculate-forging-info.js");
                    const info = calculateForgingInfo(safeInteger(timestamp, "timestamp"), safeInteger(height, "height"), noLookup);
                    return {
                        currentForger: info.currentForger,
                        nextForger: info.nextForger,
                        blockTimestamp: info.blockTimestamp,
                        canForge: info.canForge,
                    };
                }),
            ),

        // previous {id, height, timestamp}, next {id, previousBlock, height, timestamp}. The reason
        // is the check that refuses the pair, in the reference's order; the slots are given when
        // it is the slot check.
        "block.isChained": ({ previous, next, milestones }, ruleHeight) =>
            withConfig(milestones, () =>
                atHeight(ruleHeight, () => {
                    const { isBlockChained, getBlockNotChainedErrorMessage } = util("is-block-chained.js");
                    const a = blockData(previous, "previous");
                    const b = blockData(next, "next");
                    if (isBlockChained(a, b, noLookup)) return { isChained: true };
                    const message = getBlockNotChainedErrorMessage(a, b, noLookup);
                    if (message.endsWith(": previous block id mismatch")) return { isChained: false, reason: "previousBlock" };
                    if (message.endsWith(": height is not plus one")) return { isChained: false, reason: "height" };
                    const slots = /: previous slot is not smaller: (-?\d+) \(derived from timestamp .*\) VS (-?\d+) \(derived from timestamp .*\)$/.exec(message);
                    if (!slots) throw new Error(`unrecognised chaining message: ${message}`);
                    return { isChained: false, reason: "slot", previousSlot: Number(slots[1]), nextSlot: Number(slots[2]) };
                }),
            ),

        "kernel.stringify": ({ value }) => ({ text: solar.stringify(value) }),

        "kernel.dotenv": ({ text }) => {
            if (typeof text !== "string") throw new TypeError("text must be a string");
            const { dotenv } = util("dot-env.js");
            const pairs = dotenv.parse(text);
            return { pairs: Object.entries(pairs), text: dotenv.stringify(pairs) };
        },

        "kernel.formatTimestamp": ({ timestamp, milestones }, ruleHeight) =>
            withConfig(milestones, () =>
                atHeight(ruleHeight, () => {
                    const { formatTimestamp } = util("format-timestamp.js");
                    const { epoch, unix, human } = formatTimestamp(safeInteger(timestamp, "timestamp"));
                    return { epoch, unix, human };
                }),
            ),

        "kernel.ip": ({ list, ip }) => {
            if (!Array.isArray(list) || !list.every((entry) => typeof entry === "string")) throw new TypeError("list must be an array of strings");
            if (typeof ip !== "string") throw new TypeError("ip must be a string");
            const { isWhitelisted } = util("is-whitelisted.js");
            const { isBlacklisted } = util("is-blacklisted.js");
            const address = util("ip-address.js");
            return {
                whitelisted: isWhitelisted(list, ip),
                blacklisted: isBlacklisted(list, ip),
                valid: address.isValidAddress(ip),
                ipv6: address.isIPv6Address(ip),
                normalised: address.normaliseAddress(ip),
                cleaned: address.cleanAddress(ip),
            };
        },

        // RegisterBasePaths for the application token and network, without its side effects (it
        // creates the directories and exports CORE_PATH_*): env-paths with suffix "core", each path
        // replaced by its CORE_PATH_* variable or followed by the network, then expandTilde and
        // path.resolve. The call runs with exactly the variables of `env`, HOME set to `home` and
        // the working directory `cwd` (an existing directory, "/" by default); env-paths is loaded
        // afresh, because it reads the home and temp directories when it is loaded.
        "kernel.paths": ({ token, network, env = {}, home, cwd = "/" }) => {
            for (const [name, value] of [["token", token], ["network", network], ["home", home], ["cwd", cwd]]) {
                if (typeof value !== "string") throw new TypeError(`${name} must be a string`);
            }
            if (home === "") throw new TypeError("home must not be empty");
            if (env === null || typeof env !== "object" || Array.isArray(env)) throw new TypeError("env must be an object");
            for (const [name, value] of Object.entries(env)) {
                if (!PATH_VARIABLES.includes(name) || name === "HOME") throw new TypeError(`env may not set ${name}`);
                if (typeof value !== "string") throw new TypeError(`env.${name} must be a string`);
            }
            const envPathsFile = require.resolve("env-paths", { paths: [kernelDir] });
            const saved = Object.fromEntries(PATH_VARIABLES.map((name) => [name, process.env[name]]));
            const savedCwd = process.cwd();
            try {
                for (const name of PATH_VARIABLES) delete process.env[name];
                Object.assign(process.env, env, { HOME: home });
                process.chdir(cwd);
                delete require.cache[envPathsFile];
                const envPaths = require(envPathsFile);
                const { expandTilde } = util("expand-tilde.js");
                const out = {};
                for (let [type, dir] of Object.entries(envPaths(token, { suffix: "core" }))) {
                    const processPath = process.env[`CORE_PATH_${type.toUpperCase()}`];
                    dir = processPath ? processPath : `${dir}/${network}`;
                    out[type] = path.resolve(expandTilde(dir));
                }
                return out;
            } finally {
                delete require.cache[envPathsFile];
                process.chdir(savedCwd);
                for (const name of PATH_VARIABLES) {
                    if (saved[name] === undefined) delete process.env[name];
                    else process.env[name] = saved[name];
                }
            }
        },

        // One message through PinoLogger.log (the empty-message check and node-emoji, with the
        // plugin's `emojify` setting), a pino instance configured as make() configures it with
        // the time fixed, and the pretty transport of make(), coloured or not. The transport's
        // own level passes every level. `line` is null when nothing is written. The driver's
        // level styles use chalk's default instance, whose level is fixed at 0 here, as it is in
        // a node process without a terminal.
        "logger.format": ({ level, time, message, colorize, emojify }) => {
            if (!Object.prototype.hasOwnProperty.call(LEVELS, level)) throw new TypeError(`unknown level ${level}`);
            safeInteger(time, "time");
            if (typeof message !== "string") throw new TypeError("message must be a string");
            if (typeof colorize !== "boolean" || typeof emojify !== "boolean") throw new TypeError("colorize and emojify must be booleans");
            const { pino, chalk, PinoLogger } = loggerParts();
            const chalkLevel = chalk.level;
            chalk.level = 0;
            try {
                const written = [];
                const stream = { write: (text) => written.push(text) };
                const instance = new PinoLogger();
                instance.configuration = { get: (key) => (key === "emojify" ? emojify : undefined) };
                instance.logger = pino(
                    {
                        base: null,
                        customLevels: LEVELS,
                        level: "emergency",
                        formatters: { level: (label) => ({ level: label }) },
                        useOnlyCustomLevels: true,
                        safe: true,
                        timestamp: () => `,"time":${time}`,
                    },
                    stream,
                );
                instance.log(level, message);
                if (written.length === 0) return { line: null };
                if (written.length !== 1) throw new Error(`pino wrote ${written.length} chunks`);
                const transport = instance.createPrettyTransport("debug", { colorize });
                const lines = [];
                for (const chunk of written[0].split("\n").filter((part) => part !== "")) {
                    transport._transform(Buffer.from(chunk, "utf8"), "buffer", (error, out) => {
                        if (error) throw error;
                        if (out !== undefined) lines.push(String(out));
                    });
                }
                return { line: lines.length === 0 ? null : lines.join("") };
            } finally {
                chalk.level = chalkLevel;
            }
        },
    };
};
