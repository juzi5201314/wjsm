import { setTimeout, setInterval } from 'node:timers/promises';
import { fetch, Headers, structuredClone } from './shim.js';
console.log(typeof setTimeout);
console.log(await setTimeout(1, 'waited'));
console.log(fetch('/x'));
console.log(new Headers().tag);
console.log(structuredClone('v'));
let ticks = 0;
for await (const value of setInterval(1, 'tick')) {
  ticks = ticks + 1;
  console.log(value, ticks);
  if (ticks === 2) break;
}
