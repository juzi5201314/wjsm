import dc, { channel, subscribe, unsubscribe, hasSubscribers, Channel } from 'node:diagnostics_channel';
import dcBare from 'diagnostics_channel';
console.log(dc === dcBare);
console.log(dc.channel === channel, dc.Channel === Channel);
const ch = channel('fixture:test');
console.log(ch === channel('fixture:test'), ch instanceof Channel, ch.name);
console.log(ch.hasSubscribers, hasSubscribers('fixture:test'));
const handler = (message, name) => console.log('received', message.value, name);
subscribe('fixture:test', handler);
console.log(ch.hasSubscribers);
ch.publish({ value: 42 });
console.log(unsubscribe('fixture:test', handler), unsubscribe('fixture:test', handler));
console.log(ch.hasSubscribers);
const expectInvalid = (label, fn) => {
  try {
    fn();
    console.log(label, 'no error');
  } catch (e) {
    console.log(label, e instanceof TypeError, e.code, e.message);
  }
};
expectInvalid('number', () => channel(123));
expectInvalid('null', () => channel(null));
expectInvalid('undefined', () => channel(undefined));
expectInvalid('object', () => channel({}));
expectInvalid('array', () => channel([1, 2]));
expectInvalid('boolean', () => subscribe(true, handler));
expectInvalid('float', () => unsubscribe(1.5, handler));
expectInvalid('nan', () => channel(NaN));
expectInvalid('negzero', () => channel(-0));
expectInvalid('bigint', () => channel(10n));
expectInvalid('function', () => channel(function myFn() {}));
expectInvalid('nullproto', () => channel(Object.create(null)));
console.log(hasSubscribers(123), hasSubscribers(null));
const sym = Symbol('fixture:sym');
const symCh = channel(sym);
console.log(symCh === channel(sym), symCh instanceof Channel, typeof symCh.name);
const symHandler = (message, name) => console.log('sym received', message, String(name));
subscribe(sym, symHandler);
symCh.publish('sym-payload');
console.log(unsubscribe(sym, symHandler));
const direct = new Channel(77);
console.log(channel(77) === direct, direct.name, hasSubscribers(77));
