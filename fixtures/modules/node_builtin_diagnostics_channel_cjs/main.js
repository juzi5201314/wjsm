const dc = require('diagnostics_channel');
console.log(dc === require('node:diagnostics_channel'));
const ch = dc.channel('cjs:test');
ch.subscribe(message => console.log('sub got', message));
console.log(ch.hasSubscribers);
ch.publish('payload');
const direct = new dc.Channel('cjs:direct');
console.log(direct.hasSubscribers, direct.name);
