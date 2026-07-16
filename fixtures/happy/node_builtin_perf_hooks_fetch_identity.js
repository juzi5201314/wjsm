const fetchBeforePerfHooks = globalThis.fetch;

require("node:perf_hooks");

console.log(fetchBeforePerfHooks === globalThis.fetch);
