import { setTimeout as wait, setImmediate as settleNext, setInterval as every, scheduler } from 'node:timers/promises';
import tpBare from 'timers/promises';
import timers from 'node:timers';
console.log(tpBare.setTimeout === wait);
console.log(timers.promises === tpBare);
console.log(await wait(1, 'waited'));
console.log(await settleNext('yielded'));
await scheduler.wait(1);
console.log('scheduler.wait done');
await scheduler.yield();
console.log('scheduler.yield done');
let ticks = 0;
for await (const value of every(1, 'tick')) {
  ticks = ticks + 1;
  console.log(value, ticks);
  if (ticks === 2) break;
}
