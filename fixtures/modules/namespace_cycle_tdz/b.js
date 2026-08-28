import * as a from './main.js';
export const fromB = 'b';
try {
  console.log('inB', a.fromA);
} catch (e) {
  console.log('inB', e.name, e.message);
}
try {
  console.log('descInB', JSON.stringify(Object.getOwnPropertyDescriptor(a, 'fromA')));
} catch (e) {
  console.log('descInB', e.name, e.message);
}
try {
  console.log('keysInB', Object.keys(a).join(','));
} catch (e) {
  console.log('keysInB', e.name, e.message);
}
console.log('namesInB', Object.getOwnPropertyNames(a).join(','));
console.log('protoInB', Object.getPrototypeOf(a), Object.isExtensible(a), Object.prototype.toString.call(a));
console.log('reflectSetInB', Reflect.set(a, 'fromA', 1), Reflect.set(a, 'other', 1));
