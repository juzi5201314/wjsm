const dc = require('diagnostics_channel');
console.log(dc === require('node:diagnostics_channel'));
const ch = dc.channel('cjs:test');
ch.subscribe(message => console.log('sub got', message));
console.log(ch.hasSubscribers);
ch.publish('payload');
const direct = new dc.Channel('cjs:direct');
console.log(direct.hasSubscribers, direct.name);
console.log(dc.channel('cjs:direct') === direct);
try {
  dc.channel(42);
} catch (e) {
  console.log(e instanceof TypeError, e.code, e.message);
}
try {
  dc.subscribe({}, () => {});
} catch (e) {
  console.log(e.code, e.message);
}
console.log(dc.hasSubscribers(42));
const directNum = new dc.Channel(7);
console.log(dc.channel(7) === directNum, directNum.name);
