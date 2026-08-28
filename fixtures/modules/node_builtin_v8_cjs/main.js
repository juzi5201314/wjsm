const v8 = require('v8');
console.log(v8 === require('node:v8'));
console.log(typeof v8.getHeapStatistics);
const stats = v8.getHeapStatistics();
console.log(Object.keys(stats).length);
console.log(stats.used_heap_size > 0, stats.used_heap_size <= stats.heap_size_limit);
console.log(typeof v8.getHeapSpaceStatistics, typeof v8.writeHeapSnapshot, typeof v8.cachedDataVersionTag);
