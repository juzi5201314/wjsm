// Module Namespace Exotic Object（§10.4.6）全协议对拍：null 原型、不可扩展、
// 导出呈现 writable=true/configurable=false 数据描述符、[[Set]]/[[Delete]]/
// [[DefineOwnProperty]]/[[SetPrototypeOf]] 拒绝语义与 V8 文案、seal/freeze、
// live binding 与动态 import 对象身份。
import * as ns from './m.js';

function attempt(label, fn) {
  try {
    const r = fn();
    console.log(label, 'ok', typeof r === 'object' && r !== null ? '[object]' : String(r));
  } catch (e) {
    console.log(label, e.name, e.message);
  }
}

console.log('proto', Object.getPrototypeOf(ns));
console.log('extensible', Object.isExtensible(ns));
console.log('toString', Object.prototype.toString.call(ns));
const d = Object.getOwnPropertyDescriptor(ns, 'counter');
console.log('desc', d.value, d.writable, d.enumerable, d.configurable, 'get' in d);
const tag = Object.getOwnPropertyDescriptor(ns, Symbol.toStringTag);
console.log('tagDesc', tag.value, tag.writable, tag.enumerable, tag.configurable);
console.log('keys', Object.keys(ns).join(','));
console.log('names', Object.getOwnPropertyNames(ns).join(','));
console.log('symbols', Object.getOwnPropertySymbols(ns).length);
console.log('values', Object.values(ns).map((v) => typeof v).join(','));
console.log('entries', Object.entries(ns).map(([k, v]) => k + ':' + typeof v).join(','));
console.log('reflectSet', Reflect.set(ns, 'counter', 99));
console.log('reflectSetNew', Reflect.set(ns, 'zzz', 1));
attempt('strictAssign', function () { 'use strict'; ns.counter = 5; });
attempt('strictAssignNew', function () { 'use strict'; ns.zzz = 5; });
console.log('reflectDelete', Reflect.deleteProperty(ns, 'counter'));
console.log('reflectDeleteMissing', Reflect.deleteProperty(ns, 'zzz'));
attempt('strictDelete', function () { 'use strict'; delete ns.counter; });
console.log('reflectDefineSame', Reflect.defineProperty(ns, 'counter', { value: 0 }));
console.log('reflectDefineDiff', Reflect.defineProperty(ns, 'counter', { value: 1 }));
console.log('reflectDefineWritFalse', Reflect.defineProperty(ns, 'counter', { writable: false }));
console.log('reflectDefineNew', Reflect.defineProperty(ns, 'zzz', { value: 1 }));
attempt('defineDiff', () => Object.defineProperty(ns, 'counter', { value: 1 }));
attempt('defineTag', () => Object.defineProperty(ns, Symbol.toStringTag, { value: 'Module' }) === ns);
attempt('defineTagDiff', () => Object.defineProperty(ns, Symbol.toStringTag, { value: 'Nope' }));
attempt('setProtoObj', () => Object.setPrototypeOf(ns, {}));
attempt('setProtoNull', () => Object.setPrototypeOf(ns, null) === ns);
console.log('reflectSetProto', Reflect.setPrototypeOf(ns, {}), Reflect.setPrototypeOf(ns, null));
attempt('freeze', () => Object.freeze(ns));
attempt('seal', () => Object.seal(ns) === ns);
console.log('isSealed', Object.isSealed(ns), 'isFrozen', Object.isFrozen(ns));
attempt('preventExt', () => Object.preventExtensions(ns) === ns);
console.log('live0', ns.counter);
ns.bump();
console.log('live1', ns.counter, Object.getOwnPropertyDescriptor(ns, 'counter').value);
console.log('in', 'counter' in ns, 'zzz' in ns);
const dyn = await import('./m.js');
console.log('sameIdentity', dyn === ns);
console.log('spread', JSON.stringify({ ...ns }));
console.log('forIn', (() => { const ks = []; for (const k in ns) ks.push(k); return ks.join(','); })());
