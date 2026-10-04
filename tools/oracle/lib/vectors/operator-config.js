"use strict";
// V35's forger key records: `config.forgerKey` (the flags and the existing delegates.json, or none,
// and the reference's validation, its failure on an absent file, and the bytes it writes). A
// refusal is a record with the reference's error, as the configuration class records its refusals.

module.exports = async function operatorConfigVectors({ rec, shared }) {
    const out = [];
    const H = shared.height;
    const push = async (name, privateKey, delegatesJson) => {
        const input = { flags: { privateKey } };
        if (delegatesJson !== undefined) input.delegatesJson = delegatesJson;
        out.push({ ...(await rec("config.forgerKey", H, input)), name });
    };

    // Keys: 64 hexadecimal characters of any case, as the command's schema reads them.
    const key = "0123456789abcdef".repeat(4);
    const upper = key.toUpperCase();
    const mixed = "aB3dE5f7".repeat(8);
    const other = "f".repeat(64);
    const empty = '{\n    "keys": []\n}\n';

    // Accepted: the file the template gives, and the keys written whatever case they come in.
    await push("the published file's empty list", key, empty);
    await push("an upper-case key is lower-cased", upper, empty);
    await push("a mixed-case key is lower-cased", mixed, empty);
    await push("a key of digits and one letter", "1".repeat(63) + "a", empty);

    // Accepted: what is in the file besides `keys` stays, in its place.
    await push("secrets: [] stays and keys follows it", key, '{\n    "secrets": []\n}\n');
    await push("keys keeps its place after secrets", key, '{"secrets": [], "keys": []}');
    await push("keys keeps its place before secrets", key, '{"keys": ["old"], "secrets": []}');
    await push("an empty object gets keys", key, "{}");
    await push("an extra top-level key stays", key, '{"extra": 1, "keys": [], "other": {"nested": [1, 2]}}');
    await push("an extra key before and after keys", key, '{"a": "x", "keys": [], "z": true}');
    await push("a non-empty secrets list stays", key, '{"secrets": ["one two three"], "keys": []}');
    await push("old keys are replaced, not added to", key, '{"keys": ["' + other + '", "' + upper + '"]}');
    await push("keys that is not a list is replaced", key, '{"keys": "a string"}');
    await push("keys that is null is replaced", key, '{"keys": null}');
    await push("the same key as before", key, '{"keys": ["' + key + '"]}');

    // Accepted: the file is read as JavaScript reads it and written as JSON.stringify writes it.
    await push("another indentation is rewritten with four spaces", key, '{\n\t"extra": [1,\n 2],\n\t"keys": []\n}');
    await push("a compact file is spread out", key, '{"a":{"b":{"c":[]}},"d":{}}');
    await push("no final newline in the file", key, '{"keys": []}');
    await push("carriage returns in the file", key, '{\r\n    "keys": [],\r\n    "a": 1\r\n}\r\n');
    await push("a byte order mark is dropped", key, "﻿" + empty);
    await push("numbers are written as JavaScript writes them", key, '{"a": 1.0, "b": 1e21, "c": 0.10, "d": 12345678901234567890, "e": -0, "f": 1E2}');
    await push("an array index key comes first", key, '{"b": 1, "10": 2, "2": 3, "keys": []}');
    await push("a repeated key keeps its first place and last value", key, '{"keys": [], "x": 1, "keys": ["old"], "x": 2}');
    await push("escapes in strings", key, '{"s": "\\u00e9\\n\\t\\"\\\\\\/", "keys": []}');
    await push("unicode in strings", key, '{"s": "été 😀", "keys": []}');
    await push("a __proto__ key is an own key", key, '{"__proto__": {"x": 1}, "keys": []}');
    await push("a long key list", key, JSON.stringify({ keys: Array.from({ length: 53 }, (_, i) => String(i).padStart(64, "a")) }, null, 4));

    // Refused: the key.
    await push("a key of 63 characters", key.slice(1), empty);
    await push("a key of 65 characters", key + "0", empty);
    await push("an empty key", "", empty);
    await push("a key with a letter that is no digit", "g" + key.slice(1), empty);
    await push("a key that starts with 0x", "0x" + key.slice(2), empty);
    await push("a key with a space", key.slice(0, 63) + " ", empty);
    await push("a key with a line break at its end", key.slice(0, 63) + "\n", empty);
    await push("a key of full-width digits", "０".repeat(64), empty);
    await push("a key that is a number", 1234, empty);
    await push("a key that is a list", [key], empty);
    await push("a refused key is refused before the file is read", "xyz", undefined);

    // Refused: the file.
    await push("no file", key, undefined);
    await push("a file that is not JSON", key, "not json");
    await push("an empty file", key, "");
    await push("a file that is a JSON null", key, "null");
    await push("a file that is a JSON string", key, '"text"');
    await push("a file that is a JSON number", key, "42");
    await push("a file that is a JSON boolean", key, "true");
    await push("a file with a trailing comma", key, '{"keys": [],}');
    await push("a file with a comment", key, '{"keys": [] /* x */}');

    return out;
};
