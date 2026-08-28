import timers, { setTimeout as scheduleTimeout, clearTimeout as cancelTimeout, setInterval as scheduleInterval, clearInterval as cancelInterval, setImmediate as scheduleImmediate } from 'node:timers';
import timersBare from 'timers';
console.log(timers === timersBare);
console.log(timers.setTimeout === scheduleTimeout);
const cancelled = scheduleTimeout(() => console.log('cancelled fired'), 5);
cancelTimeout(cancelled);
scheduleImmediate(v => console.log('immediate', v), 'first');
scheduleTimeout((a, b) => console.log('timeout', a, b), 1, 'x', 42);
scheduleTimeout((...rest) => console.log('variadic', rest.length, rest.join(',')), 1, 1, 2, 3, 4, 5, 6);
scheduleTimeout((u, v) => console.log('explicit undefined', u === undefined, v), 1, undefined, 'kept');
const handle = scheduleInterval(v => {
  console.log('interval', v);
  cancelInterval(handle);
}, 1, 'tick');
const variadicHandle = scheduleInterval((...rest) => {
  console.log('interval variadic', rest.join('|'));
  cancelInterval(variadicHandle);
}, 1, 'a', 'b', 'c', 'd', 'e');
