"use strict";
// V28, the API: api.parseQuery over the query grammar (lists, nesting, decoding, key order, the
// names the plugins drop, the queries on which they throw, the 1,000-pair limit) and api.paginate
// over the paginated routes' queries (pages, limits, offsets, orders, criteria and refused values,
// at counts on either side of a page's end).

const PATHS = ["/api/transactions", "/api/blocks/10/transactions", "/api/wallets/genesis_1/transactions", "/api/peers", "/api/transactions/unconfirmed"];

const PARSE = [
    "", "a=1", "a=1&b=2", "a=1&a=2", "a=1&a=2&a=3", "a=1,2", "a=,", "a=,,", "a=1,2&a=3", "a=1&a=2,3", "a=1&a=,", "a=,&a=1",
    "a.b=1", "a.b.c=1", "a.b=1&a.c=2", "a=1&a.b=2", "a.b=2&a=1", "a.b=1,2", "a.=1", ".a=1", ".=1", "a..b=1", "a.b=1&a.b.c=2",
    "__proto__=1", "__proto__.x=1", "a.__proto__=1", "constructor=1", "a.prototype.b=1", "toString=1", "a.toString=1", "hasOwnProperty=1",
    "b=1&2=x&1=y", "10=a&9=b&x=c", "01=a&1=b", "4294967295=a&4294967294=b", "a.2=x&a.1=y&a.b=z",
    "a=%20", "a=+", "a=1+2", "a%2Eb=1", "a=%2C", "a=1%2C2", "a=%zz", "a=%4+1", "a=%E2%82%AC", "a=%FF", "a=%C0%80", "a=%ED%A0%80",
    "%61=1", "a=%", "a=%2", "a=b=c", "=x", "=", "a", "a=", "&", "&&a=1&&", "a=1&", "+=1", "a+b=1", "a=%25", "a=%2525", "a%20b=1",
    "a=1,2&a.0=x", "a=1,2&a.5=x", "a=1,2&a.length=1", "a=1,2&a.b=3", "a=1,2&a.01=x", "a=1,2&a.1.b=3",
    "orderBy=timestamp:desc,sequence:desc", "amount.from=1&amount.to=100", "memo=%25abc%25", "asset.votes.genesis_1=100",
    "address=a,b,c&page=2&limit=10", "transform=false", "memo=x%26y", "version=%3E%3D4.3.0",
];

const QUERIES = [
    "", "page=2", "limit=10", "page=2&limit=10", "offset=5", "offset=0&page=3&limit=10", "offset=5&page=3&limit=10", "page=999", "limit=100",
    "limit=0", "limit=101", "limit=-1", "limit=1.5", "limit=abc", "page=0", "offset=-1", "limit=05&page=02", "limit= 5 &page=2", "limit=1e1&page=2",
    "limit=5&limit=6", "limit=1,2", "foo=1", "=x", "a=1&a=,", "__proto__=1&page=2", "transform=FALSE&page=2", "transform=1",
    "orderBy=amount:asc&page=2", "orderBy=timestamp:desc,sequence:desc&limit=5", "orderBy=Amount:asc", "orderBy=amount:ASC", "orderBy=amount",
    "orderBy=timestamp:asc&orderBy=sequence:asc&page=2&limit=5", "amount.from=1&amount.to=5&page=2", "amount.foo=1", "address=a,b",
    "memo=x%26y&page=2&limit=1", "asset.votes.x=1&page=2", "type=2&typeGroup=2&page=2&limit=1", "nonce.from=1&limit=2&page=3",
    "ip=192.0.2.1&page=2&limit=1", "ip=192.0.2.0/24", "ip=notanip", "version=%3E%3D4.3.0&limit=1&page=2", "version=4.3.1,4.3.0",
];

const TOTALS = [0, 1, 5, 101, 149];

module.exports = async function apiVectors({ rec, shared }) {
    const out = [];
    const H = shared.height;
    for (const query of PARSE) out.push({ ...(await rec("api.parseQuery", H, { query })), name: `parseQuery: ${JSON.stringify(query)}` });
    const pairs = Array.from({ length: 1001 }, (_, i) => `k${i}=${i}`);
    for (const [name, query] of [
        ["1,000 pairs", pairs.slice(0, 1000).join("&")],
        ["1,001 pairs", pairs.join("&")],
        ["999 empty pairs then one", "&".repeat(999) + "a=1&b=2"],
        ["1,000 empty pairs then one", "&".repeat(1000) + "a=1"],
    ])
        out.push({ ...(await rec("api.parseQuery", H, { query })), name: `parseQuery: ${name}` });
    for (const path of PATHS) {
        for (const query of QUERIES) {
            for (const total of query === "" ? TOTALS : [149]) {
                out.push({ ...(await rec("api.paginate", H, { path, query, total })), name: `paginate: ${path}?${query} of ${total}` });
            }
        }
    }
    for (const [path, query] of [["/api/wallets/UPPER/transactions", ""], ["/api/wallets/dNkDoLLMAzV8xUtxjjkmY9WG5qwA3Gxki2/transactions", "page=2"], ["/api/blocks/abc/transactions", "limit=1"], ["/api/transactions/", "page=2"]])
        out.push({ ...(await rec("api.paginate", H, { path, query, total: 5 })), name: `paginate: ${path}?${query} of 5` });
    return out;
};
