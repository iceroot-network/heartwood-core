"use strict";
// Fixed inputs shared by the vector classes. Every passphrase here is a public test value.

const PASSPHRASES = [
    "this is a top secret passphrase",
    "differential test sender",
    "delegate one",
    "",
    "ünïcödé pässphrase ☃ 日本語",
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
];

module.exports = {
    PASSPHRASES,
    // The sender of most transactions, a second passphrase, a doubly signed sender and the block
    // generator.
    SENDER: "differential test sender",
    SECOND: "differential test second passphrase",
    DOUBLE_SENDER: "second signature sender",
    GENERATOR: "delegate one",
    OTHER: "this is a top secret passphrase",
    IPFS_HASH: "QmR45FmbVVrixReBwJkhEKde2qwHYaQzGxu4ZoDeswuF9w",
};
