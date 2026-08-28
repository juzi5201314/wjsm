import v8Default, { getHeapStatistics } from 'node:v8';

const stats = getHeapStatistics();
// 键集合与 Node 逐字节一致；数值断言只验证真实计量的不变量，保持确定性。
console.log(Object.keys(stats).join(','));
console.log(typeof stats.used_heap_size === 'number' && stats.used_heap_size > 0);
console.log(typeof stats.heap_size_limit === 'number' && stats.heap_size_limit > 0);
console.log(stats.used_heap_size <= stats.total_heap_size);
console.log(stats.does_zap_garbage, stats.number_of_detached_contexts);
console.log(typeof v8Default.getHeapStatistics, v8Default.getHeapStatistics === getHeapStatistics);
console.log(typeof v8Default.serialize, typeof v8Default.deserialize, typeof v8Default.setFlagsFromString);
