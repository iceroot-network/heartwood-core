"use strict";
// Oracle operations of the node's configuration:
//   config.pluginOptions {package, options, env} -> the options after the package's configSchema
//                                                   and defaults, or the error
//   config.loadApp       {app, role}             -> the role's profile of the app.json text, or
//                                                   the error
// Loaded by lib/ops.js. Until an operation is written it throws, and the harness reports its
// records as having no Rust implementation.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = function configOps() {
    return {
        "config.pluginOptions": notWritten("config.pluginOptions"),
        "config.loadApp": notWritten("config.loadApp"),
    };
};
