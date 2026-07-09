// 轻量：不 fork 真子进程，只验证 isPrimary / 模块加载
const cluster = require('cluster');
const nodeCluster = require('node:cluster');
console.log(cluster === nodeCluster);
console.log(cluster.isPrimary === true);
console.log(cluster.isWorker === false);
console.log(cluster.isMaster === true);
console.log(typeof cluster.fork);
console.log(typeof cluster.Worker);
console.log(cluster.SCHED_RR === 2);
console.log(cluster.SCHED_NONE === 1);
process.exit(0);
