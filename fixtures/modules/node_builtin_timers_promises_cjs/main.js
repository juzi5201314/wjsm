const tp = require('timers/promises');
console.log(tp === require('node:timers/promises'));
console.log(typeof tp.setTimeout, typeof tp.setImmediate, typeof tp.setInterval, typeof tp.scheduler.wait);
tp.setTimeout(1, 'first').then(v => console.log('then', v));
tp.setImmediate('imm').then(v => console.log('immediate', v));
console.log('sync end');
