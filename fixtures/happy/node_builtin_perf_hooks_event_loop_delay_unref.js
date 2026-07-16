const { monitorEventLoopDelay } = require("node:perf_hooks");

const delay = monitorEventLoopDelay({ resolution: 1 });
console.log(delay.enable() === true);
