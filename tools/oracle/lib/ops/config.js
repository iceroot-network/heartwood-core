"use strict";
// Oracle operations of the node's configuration:
//   config.pluginOptions {package, options, env} -> what the reference's kernel makes of a plugin
//                        entry's options: the package's defaults (dist/defaults.js, loaded with
//                        the process environment set to `env`) merged with `options` as
//                        PluginConfiguration.merge does (deepmerge, arrays replaced), checked
//                        with the package's configSchema() as RegisterServiceProviders
//                        validateConfiguration does, and merged with the validated value. The
//                        output is {options}, the part of the options a relay reads (PROJECTIONS),
//                        or the error InvalidPluginConfiguration, naming the first refused key
//   config.pluginStart   {package, options, env} -> {next}: what the process does next with those
//                        options, as RegisterServiceProviders decides: "runs" when the schema
//                        accepts them; when it refuses them, "start refused" for a package whose
//                        service provider is required() (ServiceProviderCannotBeRegistered), else
//                        "left out" (the provider is marked failed and the node runs without it)
//   config.loadApp       {app, role} -> {plugins: [{package, options}]}: LocalConfigLoader
//                        loadApplication on the app.json text `app` for the process type `role`,
//                        then each plugin's package as LoadServiceProviders finds it
//                        (PluginDiscoverer.get over the reference's packages, and a
//                        ServiceProvider export); an entry without options gets {}; the error
//                        when either step refuses
// Loaded by lib/ops.js.

const path = require("path");

// The packages whose options a relay reads, and what it reads of each: the projection of the
// validated options that Heartwood's subject writes from its options types.
const PROJECTIONS = {
    "@solar-network/p2p": (o, env) => ({
        port: o.server.port,
        minimumVersions: o.minimumVersions,
        minimumNetworkReach: o.minimumNetworkReach,
        verifyTimeout: o.verifyTimeout,
        getBlocksTimeout: o.getBlocksTimeout,
        maxSameSubnetPeers: o.maxSameSubnetPeers,
        maxPeerSequentialErrors: o.maxPeerSequentialErrors,
        whitelist: o.whitelist,
        blacklist: o.blacklist,
        rateLimit: o.rateLimit,
        rateLimitPostTransactions: o.rateLimitPostTransactions,
        networkStart: !!o.networkStart,
        disableDiscovery: !!o.disableDiscovery,
        skipDiscovery: !!o.skipDiscovery,
        ignoreMinimumNetworkReach: !!o.ignoreMinimumNetworkReach,
        // is-valid-peer.ts reads the variable when it loads.
        allowPrivateIpRanges: !!env.CORE_P2P_ALLOW_PRIVATE_IP_RANGES,
    }),
    "@solar-network/blockchain": (o) => ({
        databaseRollback: { maxBlockRewind: o.databaseRollback.maxBlockRewind, steps: o.databaseRollback.steps },
        missedBlocksLookback: o.missedBlocksLookback,
        networkStart: !!o.networkStart,
    }),
    // The worker count depends on the host's processors, so it is left out.
    "@solar-network/pool": (o) => ({
        enabled: o.enabled,
        storage: o.storage,
        maxTransactionsInPool: o.maxTransactionsInPool,
        maxTransactionsPerSender: o.maxTransactionsPerSender,
        allowedSenders: o.allowedSenders,
        maxTransactionsPerRequest: o.maxTransactionsPerRequest,
        maxTransactionAge: o.maxTransactionAge,
        maxTransactionBytes: o.maxTransactionBytes,
        dynamicFees: {
            enabled: o.dynamicFees.enabled,
            minFeePool: o.dynamicFees.minFeePool,
            minFeeBroadcast: o.dynamicFees.minFeeBroadcast,
            addonBytes: o.dynamicFees.addonBytes,
        },
    }),
    "@solar-network/api": (o) => {
        const level = ({ concurrency, queueLimit, maxOffset }) =>
            maxOffset === undefined ? { concurrency, queueLimit } : { concurrency, queueLimit, maxOffset };
        const levels = (kind) => ({ levelOne: level(kind.levelOne), levelTwo: level(kind.levelTwo) });
        const { rateLimit, semaphore } = o.plugins;
        return {
            enabled: o.server.http.enabled,
            host: o.server.http.host,
            port: o.server.http.port,
            https: o.server.https.enabled,
            rateLimit: {
                enabled: rateLimit.enabled,
                points: rateLimit.points,
                duration: rateLimit.duration,
                whitelist: rateLimit.whitelist,
                blacklist: rateLimit.blacklist,
            },
            semaphore: { enabled: semaphore.enabled, database: levels(semaphore.database), memory: levels(semaphore.memory) },
            socketTimeout: o.plugins.socketTimeout,
            whitelist: o.plugins.whitelist,
            trustProxy: o.plugins.trustProxy,
            basePath: o.options.basePath,
            estimateTotalCount: o.options.estimateTotalCount,
        };
    },
    "@solar-network/database": (o) => ({ apiConnectionTimeout: o.apiConnectionTimeout }),
};

module.exports = function configOps({ solar }) {
    const packages = path.join(solar.solarDir, "packages");
    const kernel = (file) => require(path.join(packages, "kernel", "dist", file));

    /** Run fn with the process's CORE_ variables and USER set to exactly those of `env`. */
    function withEnv(env, fn) {
        const saved = { ...process.env };
        const ours = (name) => name.startsWith("CORE_") || name === "USER";
        for (const name of Object.keys(process.env)) if (ours(name)) delete process.env[name];
        for (const [name, value] of Object.entries(env ?? {})) {
            if (!ours(name) || typeof value !== "string") throw new TypeError(`env.${name} must be a CORE_ variable or USER, as text`);
            process.env[name] = value;
        }
        try {
            return fn();
        } finally {
            for (const name of Object.keys(process.env)) if (!(name in saved)) delete process.env[name];
            Object.assign(process.env, saved);
        }
    }

    /** A package's module, loaded afresh (its defaults read the environment when they load). */
    function fresh(name, file) {
        const resolved = require.resolve(path.join(packages, name.replace("@solar-network/", ""), "dist", file));
        delete require.cache[resolved];
        return require(resolved);
    }

    /** config.pluginOptions: the validated options' projection, or InvalidPluginConfiguration. */
    function pluginOptions({ package: name, options, env }) {
        const project = PROJECTIONS[name];
        if (!project) throw new TypeError(`package must be one of ${Object.keys(PROJECTIONS).join(", ")}`);
        if (options !== undefined && (options === null || typeof options !== "object" || Array.isArray(options))) {
            throw new TypeError("options must be an object");
        }
        return withEnv(env, () => {
            const { defaults } = fresh(name, "defaults.js");
            const { ServiceProvider } = fresh(name, "service-provider.js");
            const { PluginConfiguration } = kernel("providers/plugin-configuration.js");
            const { JoiValidator } = kernel("services/validation/drivers/joi.js");
            const { InvalidPluginConfiguration } = kernel("exceptions/plugins.js");
            // LoadServiceProviders: from(name, defaults) (no global plugin options), then merge.
            const config = Object.create(PluginConfiguration.prototype);
            config.items = defaults;
            config.merge(options);
            // RegisterServiceProviders.validateConfiguration.
            const schema = Object.create(ServiceProvider.prototype).configSchema();
            const validator = new JoiValidator();
            validator.validate(config.all(), schema);
            if (validator.fails()) {
                const first = validator.resultError[0];
                throw new InvalidPluginConfiguration(name, `${first.path.join(".")}: ${first.message}`);
            }
            config.merge(validator.valid() || {});
            return { options: JSON.parse(JSON.stringify(project(config.all(), process.env))) };
        });
    }

    return {
        "config.pluginOptions": pluginOptions,

        "config.pluginStart": async (args) => {
            const { InvalidPluginConfiguration } = kernel("exceptions/plugins.js");
            try {
                pluginOptions(args);
                return { next: "runs" };
            } catch (error) {
                if (!(error instanceof InvalidPluginConfiguration)) throw error;
            }
            // RegisterServiceProviders: the provider's required(), with the package's manifest
            // set as LoadServiceProviders sets it (the default required() reads it).
            const { ServiceProvider } = fresh(args.package, "service-provider.js");
            const { PluginManifest } = kernel("providers/plugin-manifest.js");
            const provider = Object.create(ServiceProvider.prototype);
            provider.setManifest(new PluginManifest().discover(path.join(packages, args.package.replace("@solar-network/", ""))));
            return { next: (await provider.required()) ? "start refused" : "left out" };
        },

        "config.loadApp": ({ app, role }) => {
            if (typeof app !== "string" || typeof role !== "string") throw new TypeError("app and role must be text");
            const { LocalConfigLoader } = kernel("services/config/drivers/local.js");
            const { ConfigRepository } = kernel("services/config/repository.js");
            const { JoiValidator } = kernel("services/validation/drivers/joi.js");
            const { PluginDiscoverer } = kernel("providers/plugin-discoverer.js");
            const { Identifiers } = kernel("ioc/index.js");
            const repository = new ConfigRepository();
            const application = {
                get: (id) => {
                    if (id !== Identifiers.ConfigFlags) throw new Error("only the flags are bound");
                    return { processType: role };
                },
                dataPath: (file = "") => path.join("/nonexistent-heartwood-oracle", file),
            };
            const loader = Object.create(LocalConfigLoader.prototype);
            Object.assign(loader, { app: application, configRepository: repository, validationService: new JoiValidator() });
            // loadFromLocation(["app.json", "app.js"]) with the text as app.json.
            loader.loadFromLocation = () => JSON.parse(app);
            loader.loadApplication();
            const discoverer = Object.create(PluginDiscoverer.prototype);
            discoverer.app = application;
            return (async () => {
                await discoverer.initialise();
                const plugins = repository.get("app.plugins").map((plugin) => {
                    const { packageId } = discoverer.get(plugin.package);
                    if (typeof require(packageId).ServiceProvider !== "function") {
                        throw new Error(`${plugin.package} has no service provider`);
                    }
                    return { package: plugin.package, options: plugin.options ?? {} };
                });
                return { plugins };
            })();
        },
    };
};
