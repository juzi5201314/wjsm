import assert, { equal, deepEqual, notEqual, strictEqual, deepStrictEqual, ok, throws, strict } from 'node:assert/strict';
import assertBare from 'assert/strict';
import baseAssert from 'node:assert';
console.log(assert === assertBare);
console.log(strict === assert, assert.strict === assert);
console.log(equal === baseAssert.strictEqual, deepEqual === baseAssert.deepStrictEqual);
assert(true);
ok(1);
equal(2, 2);
strictEqual('a', 'a');
notEqual(1, 2);
deepEqual({ list: [1, 2] }, { list: [1, 2] });
deepStrictEqual(new Map([['k', 1]]), new Map([['k', 1]]));
throws(() => {
  throw new RangeError('boom');
});
let coercionRejected = false;
try {
  equal(1, '1');
} catch (e) {
  coercionRejected = e.name === 'AssertionError';
}
console.log(coercionRejected);
console.log('assert strict done');
