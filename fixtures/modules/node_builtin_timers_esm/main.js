import timers, { setTimeout as scheduleTimeout, clearTimeout as cancelTimeout, setInterval as scheduleInterval, clearInterval as cancelInterval, setImmediate as scheduleImmediate } from 'node:timers';
import timersBare from 'timers';
console.log(timers === timersBare);
console.log(timers.setTimeout === scheduleTimeout);
const cancelled = scheduleTimeout(() => console.log('cancelled fired'), 5);
cancelTimeout(cancelled);
scheduleImmediate(v => console.log('immediate', v), 'first');
scheduleTimeout((a, b) => console.log('timeout', a, b), 1, 'x', 42);
const handle = scheduleInterval(v => {
  console.log('interval', v);
  cancelInterval(handle);
}, 1, 'tick');
