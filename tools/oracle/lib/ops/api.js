"use strict";
// Oracle operations of the API's query grammar and pagination, over the reference's own server
// modules (packages/api/dist) and its hapi:
//   api.parseQuery {query}               -> {query}: the query string as the reference's server
//                                           reads it (hapi parses a request's relative URL with
//                                           Node's querystring.parse into a null-prototype object,
//                                           request.js _parseUrl and _parseQuery), then its
//                                           comma-array and dot-nesting plugins
//                                           (plugins/comma-array-query.ts, dot-separated-query.ts);
//                                           {status: 500} when a plugin throws, which the server
//                                           answers with 500
//   api.paginate   {path, query, total}  -> {meta, query}: GET path?query on a hapi server with the
//                                           reference's routes (handlers.ts, routes/*.ts, their
//                                           validation and failAction as server.ts sets them), its
//                                           query plugins and its pagination plugin
//                                           (plugins/pagination), whose controllers answer the page
//                                           of `total` matches that Controller.getListingPage
//                                           picks; query is the validated query the handler reads;
//                                           {status} when the server refuses the request
// Loaded by lib/ops.js.

const path = require("path");
const Querystring = require("querystring");

module.exports = function apiOps({ solar }) {
    const apiDir = path.join(solar.solarDir, "packages", "api");
    const api = (...parts) => require(path.join(apiDir, "dist", ...parts));
    const dependency = (name) => require(require.resolve(name, { paths: [apiDir] }));

    function queryPlugins() {
        return [api("plugins", "comma-array-query").commaArrayQuery, api("plugins", "dot-separated-query").dotSeparatedQuery];
    }

    function parseQuery({ query }) {
        if (typeof query !== "string") throw new TypeError("query must be text");
        const request = { query: Object.assign(Object.create(null), Querystring.parse(query)) };
        const h = { continue: Symbol("continue") };
        try {
            for (const plugin of queryPlugins()) plugin.onRequest(request, h);
        } catch {
            return { status: 500 };
        }
        return { query: JSON.parse(JSON.stringify(request.query)) };
    }

    // One server for every api.paginate call; each call sets the count its controllers answer.
    let server;
    const call = { total: 0, query: undefined };

    async function paginationServer() {
        if (server) return server;
        const Hapi = dependency("@hapi/hapi");
        const Boom = dependency("@hapi/boom");
        const { Controller } = api("controllers", "controller");
        const list = (request) => {
            call.query = request.query;
            const page = Controller.prototype.getListingPage.call({}, request);
            const count = Math.max(0, Math.min(page.limit, call.total - page.offset));
            return { results: Array(count).fill(null), totalCount: call.total, meta: { totalCountIsEstimate: false } };
        };
        const controller = new Proxy({}, { get: () => list });
        const failAction = async (request, h, err) => Boom.badData(err.message);
        server = new Hapi.Server({ router: { stripTrailingSlash: true }, routes: { payload: { failAction }, validate: { failAction } } });
        server.app.app = {
            resolve: () => controller,
            getTagged: () => ({ get: () => 40, getRequired: () => 40, getOptional: (key, fallback) => fallback }),
        };
        server.app.schemas = api("schemas");
        await server.register(queryPlugins().map((plugin) => ({ plugin })));
        await server.register({ plugin: api("plugins", "pagination").pagination });
        await server.register({ plugin: api("handlers"), routes: { prefix: "/api" } });
        return server;
    }

    async function paginate({ path: requestPath, query, total }) {
        if (typeof requestPath !== "string" || !requestPath.startsWith("/api/")) throw new TypeError("path must start with /api/");
        if (typeof query !== "string") throw new TypeError("query must be text");
        if (!Number.isSafeInteger(total) || total < 0) throw new TypeError("total must be a count");
        const s = await paginationServer();
        call.total = total;
        call.query = undefined;
        const response = await s.inject({ method: "GET", url: query ? `${requestPath}?${query}` : requestPath });
        if (response.statusCode !== 200) return { status: response.statusCode };
        return { meta: JSON.parse(response.payload).meta, query: JSON.parse(JSON.stringify(call.query)) };
    }

    return {
        "api.parseQuery": parseQuery,
        "api.paginate": paginate,
    };
};
