const timers = require('timers');
console.log(timers === require('node:timers'));
console.log(typeof timers.promises.setTimeout);
const cancelled = timers.setTimeout(() => console.log('cancelled fired'), 5);
timers.clearTimeout(cancelled);
timers.setTimeout(v => console.log('cjs timeout', v), 1, 'ok');
const handle = timers.setInterval(v => {
  console.log('cjs interval', v);
  timers.clearInterval(handle);
}, 1, 'tick');
