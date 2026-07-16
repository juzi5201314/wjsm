const perfHooks = require('node:perf_hooks');
const path = require('node:path');

console.log(typeof perfHooks.performance.now, path.join('a', 'b'));
