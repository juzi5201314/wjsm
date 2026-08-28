const proc = require('process');
console.log(proc === require('node:process'));
console.log(proc === process);
console.log(typeof proc.pid === 'number', typeof proc.env === 'object');
console.log(Array.isArray(proc.argv));
proc.nextTick(v => console.log('tick', v), 'arg');
console.log('sync end');
