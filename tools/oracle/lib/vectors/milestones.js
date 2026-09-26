"use strict";
// Milestone files as JSON text, for the cases that run under milestones of their own. Values are
// written as JSON text so that a case keeps the exact spelling it is about (8.0, 8e0, -0,
// 9007199254740993); the reference reads the text with JSON.parse, as it reads a file.

// The height-1 milestone of a compact devnet: the keys the configuration checks and the codecs
// read, with the devnet's values and epoch.
const TEMPLATE = [
    ["height", "1"],
    ["activeDelegates", "53"],
    ["blockTime", "8"],
    ["block", '{"version":0,"maxTransactions":150,"maxPayload":2097152}'],
    ["blocksToRevokeDelegateResignation", "106"],
    ["epoch", '"2026-01-01T00:00:00.000Z"'],
    ["reward", "0"],
    ["burn", '{"feeBasisPoints":9000,"txAmount":2000000}'],
    ["donations", "{}"],
    ["dynamicReward", '{"enabled":false}'],
    ["transfer", '{"maximum":256,"minimum":1}'],
    ["legacyTransfer", "false"],
    ["legacyVote", "false"],
    ["acceptLegacySchnorrTransactions", "false"],
    ["bip340", "true"],
];

/** An object as JSON text from [key, JSON text] pairs, in order. */
function objectText(pairs) {
    return `{${pairs.map(([key, value]) => `${JSON.stringify(key)}:${value}`).join(",")}}`;
}

/**
 * The height-1 milestone with overrides: a key maps to its value as JSON text, or to undefined to
 * leave the key out. Keys the template lacks are appended in the order given.
 */
function firstMilestone(overrides = {}) {
    const pairs = [];
    for (const [key, value] of TEMPLATE) {
        if (!Object.prototype.hasOwnProperty.call(overrides, key)) pairs.push([key, value]);
        else if (overrides[key] !== undefined) pairs.push([key, overrides[key]]);
    }
    for (const [key, value] of Object.entries(overrides)) {
        if (!TEMPLATE.some(([k]) => k === key) && value !== undefined) pairs.push([key, value]);
    }
    return objectText(pairs);
}

/** A milestone file: the height-1 milestone with overrides, then later milestones as JSON text. */
function milestonesText(overrides = {}, later = []) {
    return `[${[firstMilestone(overrides), ...later].join(",")}]`;
}

module.exports = { TEMPLATE, firstMilestone, milestonesText, objectText };
