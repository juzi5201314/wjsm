const assert = require('assert/strict');
console.log(assert === require('node:assert/strict'));
console.log(typeof assert === 'function');
console.log(assert.equal === require('assert').strictEqual);
assert(true);
assert.equal(3, 3);
assert.deepEqual([{ a: 1 }], [{ a: 1 }]);
let coercionRejected = false;
try {
  assert.deepEqual({ n: 1 }, { n: '1' });
} catch (e) {
  coercionRejected = e.name === 'AssertionError';
}
console.log(coercionRejected);
console.log('cjs assert strict done');
