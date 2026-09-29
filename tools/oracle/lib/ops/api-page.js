"use strict";
// Oracle operations of the kernel's search services and functional-red-black-tree 1.0.1:
//   api.searchPage  {items, criteria?, sorting, pagination} -> {page: {results, totalCount, meta}}
//                   the items the standard criteria pass (all of them without criteria), through
//                   PaginationService.getPage; a throw (a comparison of mixed types, of plain
//                   objects or of a Semver that is not valid) is the operation's error, which the
//                   route answers with 500
//   js.rbTrace      {values, script} -> {compared: [[i, j], ...], keys: [i, ...]}
//                   {values, count}  -> the same for PaginationService.getTop over the values
//                   (sorted ascending, as {v: value} items), whose keys are the item indices
//                   The keys of a script are indices into `values`, which are numbers, compared as
//                   numbers; `script` lists ["insert", i] and ["remove", i]. `compared` lists the
//                   pairs the tree compares, in order.
// Items carry BigNumbers as {"$bigNumber": "<decimal>"} and Semvers as {"$semver": "<version>"}.
// Loaded by lib/ops.js.

const path = require("path");

module.exports = function apiPageOps({ solar }) {
    const kernelDir = path.join(solar.solarDir, "packages", "kernel");
    const search = (file) => require(path.join(kernelDir, "dist", "services", "search", file));
    const { BigNumber } = solar.lib.Utils;
    const { Semver } = require(path.join(kernelDir, "dist", "utils", "semver"));
    const createTree = require(require.resolve("functional-red-black-tree", { paths: [kernelDir] }));

    const markers = { $bigNumber: (text) => new BigNumber(text), $semver: (text) => new Semver(text) };

    function revive(value) {
        if (Array.isArray(value)) return value.map(revive);
        if (value === null || typeof value !== "object") return value;
        const keys = Object.keys(value);
        if (keys.length === 1 && Object.hasOwn(markers, keys[0]) && typeof value[keys[0]] === "string") return markers[keys[0]](value[keys[0]]);
        return Object.fromEntries(keys.map((key) => [key, revive(value[key])]));
    }

    function plainJson(value) {
        if (Array.isArray(value)) return value.map(plainJson);
        if (value instanceof BigNumber) return { $bigNumber: value.toString() };
        if (value instanceof Semver) return { $semver: value.toString() };
        if (value === null || typeof value !== "object") return value;
        return Object.fromEntries(Object.entries(value).map(([key, member]) => [key, plainJson(member)]));
    }

    async function searchPage({ items, criteria, sorting, pagination }) {
        if (!Array.isArray(items) || !Array.isArray(sorting)) throw new TypeError("items and sorting must be lists");
        const criterias = revive(criteria || []);
        const { StandardCriteriaService } = search("standard-criteria-service");
        const { PaginationService } = search("pagination-service");
        const criteriaService = new StandardCriteriaService();
        const matching = revive(items).filter((item) => criteriaService.testStandardCriterias(item, ...criterias));
        const page = await new PaginationService().getPage(pagination, sorting, matching);
        return { page: plainJson(page) };
    }

    async function rbTrace({ values, script, count }) {
        if (!Array.isArray(values) || !values.every((v) => typeof v === "number")) throw new TypeError("values must be numbers");
        const compared = [];
        if (script !== undefined) {
            let tree = createTree((i, j) => {
                compared.push([i, j]);
                return values[i] < values[j] ? -1 : values[i] > values[j] ? 1 : 0;
            });
            for (const [op, key] of script) {
                if (op === "insert") tree = tree.insert(key, undefined);
                else if (op === "remove") tree = tree.remove(key);
                else throw new TypeError(`unknown step ${op}`);
            }
            return { compared, keys: tree.keys };
        }
        const { PaginationService } = search("pagination-service");
        const service = new PaginationService();
        const compare = service.compare;
        service.compare = (a, b, sorting) => {
            compared.push([a.index, b.index]);
            return compare.call(service, a, b, sorting);
        };
        const top = await service.getTop([{ property: "v", direction: "asc" }], count, values.map((v, i) => ({ v, i })));
        return { compared, keys: top.map((item) => item.i) };
    }

    return { "api.searchPage": searchPage, "js.rbTrace": rbTrace };
};
