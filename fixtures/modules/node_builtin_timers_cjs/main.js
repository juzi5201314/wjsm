const timers = require('timers');
console.log(timers === require('node:timers'));
console.log(typeof timers.promises.setTimeout);
const cancelled = timers.setTimeout(() => console.log('cancelled fired'), 5);
timers.clearTimeout(cancelled);
timers.setTimeout(v => console.log('cjs timeout', v), 1, 'ok');
timers.setTimeout((...rest) => console.log('cjs variadic', rest.length, rest.join(',')), 1, 1, 2, 3, 4, 5, 6);
timers.setTimeout((u, v) => console.log('cjs explicit undefined', u === undefined, v), 1, undefined, 'kept');
const handle = timers.setInterval((...rest) => {
  console.log('cjs interval', rest.join('|'));
  timers.clearInterval(handle);
}, 1, 'tick', 'extra1', 'extra2', 'extra3', 'extra4');
